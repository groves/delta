{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer

            # native deps needed by oniguruma-sys (via bat regex-onig)
            pkg-config
            oniguruma

            # needed by git2-sys
            libgit2
            openssl

            # needed by libz-sys
            zlib

            # for gh CLI and other tools needing TLS
            cacert
          ];

          env = {
            RUST_BACKTRACE = "1";
            # Help git2-sys find libgit2
            LIBGIT2_NO_VENDOR = "1";
            # Ensure TLS works for gh and other tools
            NIX_SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          };
        };
      }
    );
}
