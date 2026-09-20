{
  description = "Local history, charts and an out-of-range alarm for an AirGradient monitor";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["x86_64-linux" "aarch64-linux"];
    forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
  in {
    nixosModules.airgradient-dashboard = ./module.nix;
    nixosModules.default = self.nixosModules.airgradient-dashboard;

    overlays.default = final: _prev: {
      airgradient-dashboard = final.callPackage ./package.nix {};
    };

    packages = forAllSystems (pkgs: {
      airgradient-dashboard = pkgs.callPackage ./package.nix {};
      default = self.packages.${pkgs.system}.airgradient-dashboard;
    });

    formatter = forAllSystems (pkgs: pkgs.alejandra);
  };
}
