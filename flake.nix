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
        import ./nix/workspace.nix {
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          inherit crane;
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
