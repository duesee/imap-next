{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-23.11";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, fenix, ... }:
    let
      inherit (nixpkgs) lib;

      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      eachSupportedSystem = lib.genAttrs supportedSystems;

      mkDevShells = system:
        let
          pkgs = import nixpkgs { inherit system; };

          rust-toolchain = fenix.packages.${system}.fromToolchainFile {
            file = ./rust-toolchain.toml;
            sha256 = "opUgs6ckUQCyDxcB9Wy51pqhd0MPGHUVbwRKKPGiwZU=";
          };

          defaultShell = pkgs.mkShell {
            buildInputs = with pkgs; [
              # Nix
              nixd
              nixpkgs-fmt

              # Rust
              rust-toolchain
              cargo-watch
            ];
          };

        in
        {
          default = defaultShell;
        };

    in
    {
      devShells = eachSupportedSystem mkDevShells;
    };
}
