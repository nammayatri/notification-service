# Services and processes required by this project
{ inputs, ... }: {
  perSystem = { config, self', pkgs, lib, system, ... }:

    {
      process-compose."notification-services" = {
        imports = [
          inputs.services-flake.processComposeModules.default
        ];
        services.redis-cluster."redis" = {
          enable = true;
        };
        services.zookeeper."zookeeper".enable = true;
        services.apache-kafka."kafka".enable = true;
      };

      # Flake outputs
      devShells.services = pkgs.mkShell {
        nativeBuildInputs = [
          config.process-compose."notification-services".outputs.package
        ];
      };
    };
}
