{
  # Inputs for the dev partition (partitions.dev in ../flake.nix). nixpkgs is
  # named nixpkgs-dev so the partition's inputs.nixpkgs still resolves to the
  # root pin.
  description = "pouncer development inputs (see partitions.dev in ../flake.nix)";

  inputs = {
    nixpkgs-dev.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs-dev";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs-dev";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs-dev";
    };
  };

  # Empty on purpose: the root flake only reads this flake's inputs.
  outputs = _: { };
}
