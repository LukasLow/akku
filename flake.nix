{
  description = "akku — host-side extra toolchains (wasm32 checks, release prep). Regular builds/tests still go through smd.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system:
        f (import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; }));
      # Keep in sync with smd.v0.6.toml (Rust 1.85, edition 2021).
      toolchain = pkgs: pkgs.rust-bin.stable."1.85.0".minimal.override {
        extensions = [ "clippy" ];
        targets = [ "wasm32-unknown-unknown" "x86_64-pc-windows-gnu" ];
      };
      # The darwin linker needs libiconv for some std/all-features builds.
      platformLibs = pkgs: pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
      # `nix shell .#release -c python3 .github/scripts/release.py 0.0.N`
      # release.py needs python3, changie, and cargo (for `cargo update`).
      releaseTools = pkgs: [ (toolchain pkgs) pkgs.python3 pkgs.changie ] ++ platformLibs pkgs;
    in
    {
      packages = forAll (pkgs: {
        default = toolchain pkgs;
        release = pkgs.buildEnv { name = "akku-release-tools"; paths = releaseTools pkgs; };
      });
      devShells = forAll (pkgs: {
        default = pkgs.mkShell { packages = [ (toolchain pkgs) ] ++ platformLibs pkgs; };
        release = pkgs.mkShell { packages = releaseTools pkgs; };
      });
    };
}
