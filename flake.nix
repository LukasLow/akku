{
  description = "akku — host-side extra toolchains (e.g. wasm32 checks). Regular builds/tests still go through smd.";

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
        targets = [ "wasm32-unknown-unknown" ];
      };
    in
    {
      packages = forAll (pkgs: { default = toolchain pkgs; });
      devShells = forAll (pkgs: {
        default = pkgs.mkShell { packages = [ (toolchain pkgs) ]; };
      });
    };
}
