{
  description = "Bach - a desktop-style UI wrapping Claude Code, Codex and opencode";

  inputs.nixpkgs.url = "nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # The backend, for the machine agents run on: `ssh <host> bach-server attach` is how the
      # desktop app reaches it, so it must be on that machine's PATH.
      packages = forAllSystems (pkgs: rec {
        bach-server = pkgs.rustPlatform.buildRustPackage {
          pname = "bach-server";
          version = (builtins.fromTOML (builtins.readFile ./crates/bach-server/Cargo.toml)).package.version;
          # Only what cargo needs: every workspace member's manifest must be there, not node_modules.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./crates ./src-tauri ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "bach-server" ];
          # The tests start real agents and servers; they run in the devShell instead.
          doCheck = false;
          meta.mainProgram = "bach-server";
        };
        default = bach-server;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.rustc
            pkgs.cargo
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.cargo-tauri
            pkgs.nodejs
            pkgs.pnpm
            pkgs.pkg-config
            # Satie's tests run real process-compose projects.
            pkgs.process-compose
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
            pkgs.libiconv
          ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            pkgs.gobject-introspection
            pkgs.openssl
            pkgs.webkitgtk_4_1
            pkgs.libsoup_3
            pkgs.gtk3
            pkgs.librsvg
            pkgs.glib-networking
            pkgs.xdotool
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules/"
            export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS"
          '';
        };
      });
    };
}
