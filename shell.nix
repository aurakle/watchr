let
  nixpkgsVer = "7b032b3c3e83368fc9abbb95f7a5d13e0a25febd";
  pkgs = import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/${nixpkgsVer}.tar.gz") { config = {}; overlays = []; };
  libs = with pkgs; [
    openssl
  ];
in pkgs.mkShell {
  name = "watchr";

  buildInputs = libs ++ (with pkgs; [
    cargo
    rustc
    gcc
    rustfmt
    pkg-config
    mpv
  ]);

  RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
  RUST_BACKTRACE = 1;
  LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath libs;
}
