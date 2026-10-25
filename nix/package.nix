{
  lib,
  stdenv,
  craneLib,
  installShellFiles,
}:

let
  commonArgs = {
    pname = "pouncer";
    inherit ((lib.importTOML ../Cargo.toml).package) version;

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../src
        ../tests
      ];
    };

    strictDeps = true;
    cargoExtraArgs = "--locked";
    # Tests run once in the dev partition's nextest check instead of on every
    # user build.
    doCheck = false;
  };

  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    nativeBuildInputs = [ installShellFiles ];
    postInstall = lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
      $out/bin/pouncer --generate man > pouncer.1
      installManPage pouncer.1
      installShellCompletion --cmd pouncer \
        --bash <($out/bin/pouncer --generate complete-bash) \
        --zsh <($out/bin/pouncer --generate complete-zsh) \
        --fish <($out/bin/pouncer --generate complete-fish)
    '';
    passthru = { inherit commonArgs cargoArtifacts; };
  }
)
