# Dev partition (partitions.dev in ../flake.nix): toolchains, shells, checks,
# formatters, hooks.
{ inputs, ... }:
{
  imports = [
    inputs.treefmt-nix.flakeModule
    inputs.git-hooks.flakeModule
  ];

  perSystem =
    {
      config,
      pkgs,
      system,
      ...
    }:
    let
      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        extensions = [
          "rust-src"
          "rust-analyzer"
          "clippy"
          "rustfmt"
        ];
      };

      # rust-overlay keys stable releases by full x.y.z, while Cargo.toml's
      # rust-version may omit the patch version.
      msrv = (pkgs.lib.importTOML ../Cargo.toml).package.rust-version;
      msrvFull = if builtins.match "[0-9]+\\.[0-9]+" msrv != null then "${msrv}.0" else msrv;
      msrvToolchain = pkgs.rust-bin.stable.${msrvFull}.default;

      pythonEnv = pkgs.python3.withPackages (ps: [ ps.pytest ]);

      # The package's toolchain rather than rustToolchain, so clippy and
      # nextest reuse the dependency build the package already produced.
      craneLib = inputs.crane.mkLib pkgs;
      inherit (config.packages.pouncer.passthru) commonArgs cargoArtifacts;
    in
    {
      _module.args.pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ inputs.rust-overlay.overlays.default ];
      };

      checks = {
        package = config.packages.pouncer;
        devShell = config.devShells.default;

        clippy = craneLib.cargoClippy (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
          }
        );

        nextest = craneLib.cargoNextest (
          commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = true;
          }
        );

        # Linting needs no MSVC linker or SDK, so the Linux and Darwin
        # runners can keep the Windows-only code compiling. Dependencies are
        # checked rather than built: a build would link crane's dummy binary.
        clippy-windows =
          let
            target = "x86_64-pc-windows-msvc";
            craneLibWindows = craneLib.overrideToolchain (rustToolchain.override { targets = [ target ]; });
            cargoArgs = "--locked --target ${target} --all-targets --all-features";
            args = commonArgs // {
              pname = "pouncer-clippy-windows";
            };
          in
          craneLibWindows.mkCargoDerivation (
            builtins.removeAttrs args [ "cargoExtraArgs" ]
            // {
              cargoArtifacts = craneLibWindows.buildDepsOnly (
                args
                // {
                  cargoBuildCommand = "cargoWithProfile check";
                  cargoExtraArgs = cargoArgs;
                }
              );
              buildPhaseCargoCommand = "cargoWithProfile clippy ${cargoArgs} -- -D warnings";
            }
          );

        # check over build: the question is whether the language and std
        # surface the code uses exists in the floor release, not codegen.
        msrv =
          let
            craneLibMsrv = craneLib.overrideToolchain msrvToolchain;
            cargoArgs = "--locked --all-targets --all-features";
            args = commonArgs // {
              pname = "pouncer-msrv";
            };
          in
          craneLibMsrv.mkCargoDerivation (
            builtins.removeAttrs args [ "cargoExtraArgs" ]
            // {
              cargoArtifacts = craneLibMsrv.buildDepsOnly (
                args
                // {
                  cargoBuildCommand = "cargoWithProfile check";
                  cargoExtraArgs = cargoArgs;
                }
              );
              buildPhaseCargoCommand = "cargoWithProfile check ${cargoArgs}";
            }
          );

        # Here rather than only on tags: a tool that broke on main would
        # otherwise surface in the middle of a release.
        tools =
          pkgs.runCommand "pouncer-tools"
            {
              nativeBuildInputs = [
                pythonEnv
                pkgs.curl
                pkgs.git
              ];
              # homebrew_test.py serves its fake GitHub on 127.0.0.1.
              __darwinAllowLocalNetworking = true;
            }
            ''
              cd ${../tools}
              pytest -p no:cacheprovider
              touch $out
            '';
      };

      treefmt = {
        projectRootFile = "flake.nix";
        # The pre-commit check already runs this wrapper through its treefmt hook.
        flakeCheck = false;
        programs.nixfmt.enable = true;
        programs.rustfmt = {
          enable = true;
          package = rustToolchain;
          edition = "2024";
        };
        programs.taplo.enable = true;
        programs.ruff-format.enable = true;
        programs.oxfmt = {
          enable = true;
          includes = [ "*.md" ];
        };
        settings.formatter.oxfmt.options = [
          "--config"
          (builtins.toFile "oxfmtrc.json" (
            builtins.toJSON {
              printWidth = 120;
              proseWrap = "always";
            }
          ))
        ];
      };

      pre-commit.settings = {
        package = pkgs.prek;
        hooks = {
          treefmt.enable = true;
          check-merge-conflicts.enable = true;
        };
      };

      devShells.default = pkgs.mkShell {
        packages = [
          rustToolchain
          pkgs.cargo-insta
          pkgs.cargo-mutants
          pkgs.cargo-nextest
          pythonEnv
          config.treefmt.build.wrapper
        ];
        shellHook = config.pre-commit.installationScript;
      };
    };
}
