{
  description = "A local diffing tool for decompilation projects";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      mkPkgs =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = mkPkgs system;
          rustToolchain = pkgs.rust-bin.stable."1.88.0".default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          mkObjdiff = pkgs.callPackage ./nix/package.nix {
            inherit rustPlatform version;
          };
        in
        rec {
          objdiff = mkObjdiff {
            crateName = "objdiff-gui";
            binaryName = "objdiff";
            withGui = true;
          };
          objdiff-cli = mkObjdiff {
            crateName = "objdiff-cli";
            binaryName = "objdiff-cli";
          };
          default = objdiff;
        }
      );

      apps = forAllSystems (system: {
        default = self.apps.${system}.objdiff;
        objdiff = {
          type = "app";
          program = "${self.packages.${system}.objdiff}/bin/objdiff";
          meta.description = "Run the objdiff graphical interface";
        };
        objdiff-cli = {
          type = "app";
          program = "${self.packages.${system}.objdiff-cli}/bin/objdiff-cli";
          meta.description = "Run the objdiff command-line interface";
        };
      });

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) objdiff objdiff-cli;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = mkPkgs system;
          rustToolchain = pkgs.rust-bin.selectLatestNightlyWith (
            toolchain:
            toolchain.default.override {
              extensions = [
                "clippy"
                "rust-src"
                "rustfmt"
              ];
              targets = [ "wasm32-wasip2" ];
            }
          );
          cargoWrapper = pkgs.writeShellScriptBin "cargo" ''
            if [[ "''${1-}" == "+nightly" ]]; then
              shift
            fi
            exec ${rustToolchain}/bin/cargo "$@"
          '';
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.objdiff ];
            packages = with pkgs; [
              cargoWrapper
              rustToolchain
              cargo-deny
              cargo-insta
              nodejs
              pre-commit
              protobuf
              rust-analyzer
            ];
            shellHook = ''
              export PATH="${cargoWrapper}/bin:$PATH"
            '';
          };
        }
      );

      formatter = forAllSystems (system: (mkPkgs system).nixfmt);
    };
}
