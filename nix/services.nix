# Services and processes required by this project
{ inputs, ... }: {
  perSystem = { config, self', pkgs, lib, system, ... }:

    {
      process-compose."notification-services" = {
        imports = [
          inputs.services-flake.processComposeModules.default
        ];
        services.redis."redis1" = {
          enable = true;
          port = 6380;
        };
      };

      # Flake outputs
      devShells.services = pkgs.mkShell {
        nativeBuildInputs = [
          config.process-compose."notification-services".outputs.package
        ];
      };
    };
}
