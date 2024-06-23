use anyhow::{bail, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    str,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufWriter},
    net::UnixListener,
    time,
};
use tokio::{net::UnixStream, process};

use clap::{Parser, Subcommand};

const SOCKET_NAME: &str = "wallswitcher.sock";
const RUNTIME_DIR_KEY: &str = "XDG_RUNTIME_DIR";

#[derive(Subcommand)]
enum Subapp {
    /// Starts daemon
    Daemon {
        /// Interval in seconds between switches
        #[arg(short, long)]
        interval: Option<usize>,

        /// Directory of wallpaper images
        directory: PathBuf,
    },

    /// Pause wallpaper switching
    Pause,

    /// Unpauses wallpaper switching
    Unpause,

    /// Kills the daemon
    Kill,
}

struct DropUnixListener {
    path: PathBuf,
    listener: UnixListener,
}

impl DropUnixListener {
    fn bind(path: &PathBuf) -> Result<DropUnixListener> {
        Ok(DropUnixListener {
            path: path.clone(),
            listener: UnixListener::bind(path)?,
        })
    }
}

impl Drop for DropUnixListener {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(_) => {
                println!("Removed {}", self.path.to_str().unwrap());
            }
            Err(e) => {
                eprintln!("Failed to remove {}: {e}", self.path.to_str().unwrap())
            }
        }
    }
}

#[derive(Parser)]
struct App {
    #[command(subcommand)]
    command: Subapp,
}

#[tokio::main]
async fn main() {
    let app = App::parse();
    match app.command {
        Subapp::Pause => {
            if let Err(e) = send("pause").await {
                eprintln!("Failed to pause wallpaper switching: {}", e)
            }
        }
        Subapp::Unpause => {
            if let Err(e) = send("unpause").await {
                eprintln!("Failed to unpause wallpaper switching: {}", e)
            }
        }
        Subapp::Kill => {
            if let Err(e) = send("kill").await {
                eprintln!("Failed to kill daemon: {}", e)
            }
        }
        Subapp::Daemon {
            interval,
            directory,
        } => {
            if let Err(e) = init(&directory, interval.unwrap_or(60)).await {
                eprintln!("Daemon error: {}", e)
            }
        }
    }
}

fn get_socket_location() -> Result<PathBuf> {
    let runtime_dir = std::env::var(RUNTIME_DIR_KEY).map(PathBuf::from)?;
    return Ok(runtime_dir.join(Path::new(SOCKET_NAME)));
}

async fn set_loop(files: Vec<PathBuf>, paused: Arc<Mutex<bool>>, interval: Duration) -> Result<()> {
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        if paused.lock().is_ok_and(|p| *p) {
            continue;
        }
        let idx = rand::random::<usize>() % files.len();
        let img = files.get(idx).unwrap();
        let out = process::Command::new("swww")
            .args(["img", "--transition-type", "fade", img.to_str().unwrap()])
            .output()
            .await?;
        if !out.status.success() {
            eprintln!(
                "swww img {} FAILED, {}",
                img.to_str().unwrap(),
                str::from_utf8(&out.stderr).unwrap()
            );
        } else {
            println!("swww img {} SUCCESS", img.to_str().unwrap(),);
        }
    }
}

async fn listen_loop(paused: Arc<Mutex<bool>>) -> Result<()> {
    let socket_path = get_socket_location()?;
    let socket = DropUnixListener::bind(&socket_path)?;
    loop {
        let (stream, _) = socket.listener.accept().await?;
        let mut reader = tokio::io::BufReader::new(stream);
        let mut buf = String::new();
        reader.read_line(&mut buf).await?;
        match buf.as_str().trim() {
            "pause" => *paused.lock().unwrap() = true,
            "unpause" => *paused.lock().unwrap() = false,
            "kill" => break,
            s => eprintln!("Unknown message received: {s}"),
        }
    }
    Ok(())
}

async fn init(dir: &PathBuf, interval: usize) -> Result<()> {
    let sigint_handler = tokio::spawn(async {
        tokio::signal::ctrl_c()
            .await
            .expect("Program exited abnormally");
        println!("Program terminated, exiting");
    });
    let sigterm_handler = tokio::spawn(async {
        let mut stream = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to set up SIGTERM handler");
        stream.recv().await;
        println!("Program received SIGTERM, exiting");
    });
    let mut daemon_cmd = process::Command::new("swww-daemon")
        .kill_on_drop(true)
        .spawn()?;
    let files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    let paused = Arc::new(Mutex::new(false));
    let listen_task = tokio::spawn(listen_loop(paused.clone()));
    let set_task = tokio::spawn(set_loop(
        files,
        paused.clone(),
        Duration::from_secs(interval as u64),
    ));
    let mut ticker = time::interval(Duration::from_millis(100));
    loop {
        ticker.tick().await;
        if sigterm_handler.is_finished() || sigint_handler.is_finished() {
            return Ok(());
        }
        if daemon_cmd.try_wait()?.is_some() {
            bail!("swww-daemon failed to run/crashed");
        }
        if listen_task.is_finished() {
            return listen_task.await?;
        }
        if set_task.is_finished() {
            return set_task.await?;
        }
    }
}

async fn send(msg: &str) -> Result<()> {
    let socket = UnixStream::connect(get_socket_location()?).await?;
    let mut writer = BufWriter::new(socket);
    let mut buf = Vec::<u8>::new();
    writeln!(&mut buf, "{msg}")?;
    writer.write_all(&buf).await?;
    writer.flush().await?;
    Ok(())
}
