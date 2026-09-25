# Nix for Rust project management
{ inputs, ... }: {
  perSystem = { config, self', pkgs, lib, system, ... }:
    let
      rustToolchain = (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml).override {
        extensions = [
          "rust-src"
          "rust-analyzer"
          "clippy"
        ];
      };
      craneLib =
        let
          base = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
        in
        base.appendCrateRegistries [
          (base.registryFromDownloadUrl {
            dl = "https://static.crates.io/crates";
            indexUrl = "https://github.com/rust-lang/crates.io-index";
          })
        ];
      args = {
        pname = "notification-service";
        src = ./..;
        buildInputs = lib.optionals pkgs.stdenv.isDarwin
          (with pkgs.darwin.apple_sdk.frameworks; [
            Security
            SystemConfiguration
            CoreServices
          ]) ++ [
          pkgs.libiconv
          pkgs.openssl
          pkgs.rdkafka
          pkgs.cyrus_sasl
        ];
        nativeBuildInputs = [
          pkgs.pkg-config
          pkgs.cmake
          pkgs.protobuf
          pkgs.grpcurl
          pkgs.redis
          pkgs.k6
        ];
        # needed to dynamically link rdkafka
        CARGO_FEATURE_DYNAMIC_LINKING = 1;
      };
      cargoArtifacts = craneLib.buildDepsOnly args;
      package = craneLib.buildPackage (args // {
        inherit cargoArtifacts;
        cargoExtraArgs = "--locked --package notification_service";
        doCheck = false; # FIXME: tests require services to be running
      });

      simPackage = name: craneLib.buildPackage (args // {
        inherit cargoArtifacts;
        pname = name;
        cargoExtraArgs = "--locked --package ${name}";
        doCheck = false; # covered by checks.sim-tests, which does not need services
        meta.mainProgram = name;
      });

      simClients = simPackage "sim-clients";
      simProducer = simPackage "sim-producer";

      check = craneLib.cargoClippy (args // {
        inherit cargoArtifacts;
        cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
      });

      simTests = craneLib.cargoTest (args // {
        inherit cargoArtifacts;
        pname = "sim";
        cargoExtraArgs = "--locked --package sim-common --package sim-clients --package sim-producer";
      });
    in
    {
      packages.default = package;
      packages.sim-clients = simClients;
      packages.sim-producer = simProducer;

      checks.clippy = check;
      checks.sim-tests = simTests;

      # Flake outputs
      devShells.rust = pkgs.mkShell {
        inputsFrom = [
          package # Makes the buildInputs of the package available in devShell (so cargo can link against Nix libraries)
        ];
        shellHook = ''
          # For rust-analyzer 'hover' tooltips to work.
          export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library";
          export DEV="true";
        '';
        nativeBuildInputs = with pkgs; [
          # Add your dev tools here.
          rustToolchain
          cargo-watch
        ];
      };
    };
}
