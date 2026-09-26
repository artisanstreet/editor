{
  description = "Artisan Editor: Cargo builds, development tools and Linux packages";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    crane.url = "github:ipetkov/crane";
  };
  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      perSystem = nixpkgs.lib.genAttrs systems (
        system:
        let
          overlays = [ rust-overlay.overlays.default ];
          pkgs = import nixpkgs { inherit system overlays; };
        in
        import ./nix/workspace.nix {
          inherit self pkgs crane;
          # Windows binaries are cross-built with the MinGW-w64 toolchain
          # (Rust target x86_64-pc-windows-gnu), which nixpkgs builds and caches.
          windowsPkgs = import nixpkgs {
            localSystem = system;
            crossSystem = pkgs.lib.systems.examples.mingwW64;
            inherit overlays;
          };
        }
      );
      collect = name: nixpkgs.lib.mapAttrs (_: value: value.${name}) perSystem;
    in
    {
      packages = collect "packages";
      apps = collect "apps";
      checks = collect "checks";
      devShells = collect "devShells";
      formatter = collect "formatter";
      nixosModules.forge = import ./nix/forge-service.nix self;
      nixosModules.default = self.nixosModules.forge;
    };
}
