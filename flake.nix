{
  description = "Air quality, weather and departure panels for a wall display";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["x86_64-linux" "aarch64-linux"];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
  in {
    nixosModules.wall-dashboards = ./module.nix;
    nixosModules.default = self.nixosModules.wall-dashboards;

    overlays.default = final: _prev: {
      wall-dashboards = final.callPackage ./package.nix {};
    };

    packages = forAllSystems (pkgs: {
      wall-dashboards = pkgs.callPackage ./package.nix {};
      default = self.packages.${pkgs.system}.wall-dashboards;
    });

    formatter = forAllSystems (pkgs: pkgs.alejandra);
  };
}
