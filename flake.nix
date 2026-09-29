{
  description = "Bach - a desktop-style UI wrapping Claude Code, Codex and opencode";

  inputs.nixpkgs.url = "nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
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
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
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
          shellHook = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
            export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules/"
            export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS"
          '';
        };
      });
    };
}
