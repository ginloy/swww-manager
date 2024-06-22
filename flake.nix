{
  inputs = {
    naersk.url = "github:nix-community/naersk/master";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    swww.url = "github:LGFae.swww";
  };

  outputs = {
    self,
    nixpkgs,
    utils,
    naersk,
    swww,
  }:
    utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {inherit system;};
        naersk-lib = pkgs.callPackage naersk {};
        lib = pkgs.lib;
      in {
        defaultPackage = naersk-lib.buildPackage {
          src = ./.;
          postInstall = ''
            wrapProgram $out/bin/wallswitcher \
              --set PATH ${lib.makeBinPath [
              swww.packages.${system}.default
            ]}
          '';
        };
        devShell = with pkgs;
          mkShell {
            buildInputs = [cargo rustc rustfmt pre-commit rustPackages.clippy];
            RUST_SRC_PATH = rustPlatform.rustLibSrc;
          };
      }
    );
}
