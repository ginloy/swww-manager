use anyhow::Result;
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
                eprintln!("Failed to start daemon: {}", e)
            }
        }
    }
}

fn get_socket_location() -> Result<PathBuf> {
    let runtime_dir = std::env::var(RUNTIME_DIR_KEY).map(|s| PathBuf::from(s))?;
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
            .args(["img", "--transition-type", "fade"])
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

async fn init(dir: &PathBuf, interval: usize) -> Result<()> {
    let socket = UnixListener::bind(get_socket_location()?)?;
    let mut daemon_cmd = process::Command::new("swww-daemon").spawn()?;
    let files = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect::<Vec<_>>();
    let paused = Arc::new(Mutex::new(false));
    let set_loop = tokio::spawn(set_loop(
        files,
        paused.clone(),
        Duration::from_secs(interval as u64),
    ));
    loop {
        let (stream, _) = socket.accept().await?;
        if daemon_cmd.try_wait().is_ok_and(|c| c.is_some()) {
            eprintln!("swww-daemon crashed, exiting");
            break;
        }
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
    set_loop.abort();
    Ok(daemon_cmd.kill().await?)
}

async fn send(msg: &str) -> Result<()> {
    let socket = UnixStream::connect(get_socket_location()?).await?;
    let mut writer = BufWriter::new(socket);
    let mut buf = Vec::<u8>::new();
    writeln!(&mut buf, "{msg}")?;
    writer.write_all(&buf).await?;
    Ok(())
}
