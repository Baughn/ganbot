{
  description = "Ganbot3 - Multi-platform bot (Discord & IRC) built in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        buildInputs = with pkgs; [
          # System dependencies
          pkg-config
          openssl

          # For image processing
          libwebp
          
          # Development tools
          rustToolchain
          cargo-watch
          cargo-edit
          cargo-outdated
          cargo-audit
          cargo-machete
          cargo-features-manager
          bacon
        ];

        nativeBuildInputs = with pkgs; [
          rustToolchain
        ];
      in
      {
        # Development shell
        devShells.default = pkgs.mkShell.override {
          stdenv = pkgs.stdenvAdapters.useMoldLinker pkgs.clangStdenv;
        } {
          inherit buildInputs nativeBuildInputs;

          # Environment variables for compilation
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
          
          # Rust environment variables
          RUST_BACKTRACE = 1;
          RUST_LOG = "ganbot3=debug";

          shellHook = ''
            echo "Ganbot3 Rust development environment"
            echo "Rust version: $(rustc --version)"
            echo ""
            echo "Available commands:"
            echo "  cargo build    - Build the project"
            echo "  cargo run      - Run the bot"
            echo "  cargo watch    - Watch for changes and rebuild"
            echo "  cargo test     - Run tests"
            echo "  cargo check    - Check for compilation errors"
            echo "  bacon          - Run bacon for continuous checking"
            echo "  cargo machete   - Remove old deps"
            echo "  cargo features prune"
            echo ""
          '';
        };

        # Package definition (for building the bot)
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "ganbot3";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = with pkgs; [ openssl ];

          # Skip tests during build (run them separately)
          doCheck = false;
        };

        # App for running the bot
        apps.default = flake-utils.lib.mkApp {
          drv = self.packages.${system}.default;
        };
      });
}
