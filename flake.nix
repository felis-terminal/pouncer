{
  description = "pouncer: terminal-agnostic single-keystroke hint picker for scrollback";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      imports = [ flake-parts.flakeModules.partitions ];

      partitionedAttrs = {
        checks = "dev";
        devShells = "dev";
        formatter = "dev";
      };
      partitions.dev = {
        extraInputsFlake = ./dev;
        extraInputs.nixpkgs-dev = inputs.nixpkgs;
        module.imports = [ ./dev/flake-module.nix ];
      };

      perSystem =
        {
          config,
          lib,
          pkgs,
          system,
          ...
        }:
        {
          _module.args.pkgs = lib.mkDefault (import inputs.nixpkgs { inherit system; });

          packages = {
            pouncer = pkgs.callPackage ./nix/package.nix {
              craneLib = inputs.crane.mkLib pkgs;
            };
            default = config.packages.pouncer;
          };
        };

      flake.overlays.default = final: _prev: {
        pouncer = final.callPackage ./nix/package.nix {
          craneLib = inputs.crane.mkLib final;
        };
      };
    };
}
