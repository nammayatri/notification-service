# Services and processes required by this project
{ inputs, ... }: {
  perSystem = { config, self', pkgs, lib, system, ... }:
    let
      monitoringPkgs = inputs.nixpkgs-monitoring.legacyPackages.${system};

      redisMasterCount = 3;
      redisMasterTargets = lib.concatMapStringsSep ", "
        (i: ''"127.0.0.1:${toString (30001 + i)}"'')
        (lib.range 0 (redisMasterCount - 1));

      prometheusConfig = pkgs.writeText "prometheus.yml" ''
        global:
          scrape_interval: 5s
          evaluation_interval: 15s

        scrape_configs:
          - job_name: notification-service
            static_configs:
              - targets: ["127.0.0.1:9091"]
          - job_name: sim-clients
            static_configs:
              - targets: ["127.0.0.1:9101"]
          - job_name: sim-producer
            static_configs:
              - targets: ["127.0.0.1:9102"]
          - job_name: redis-nodes
            metrics_path: /scrape
            static_configs:
              - targets: [${redisMasterTargets}]
            relabel_configs:
              - source_labels: [__address__]
                target_label: __param_target
              - source_labels: [__param_target]
                target_label: redis_node
              - target_label: __address__
                replacement: 127.0.0.1:9121
      '';

      grafanaDatasource = pkgs.writeText "datasource.yaml" ''
        apiVersion: 1
        datasources:
          - name: Prometheus
            type: prometheus
            uid: prometheus
            access: proxy
            url: http://127.0.0.1:9090
            isDefault: true
            jsonData:
              timeInterval: 5s
      '';

      prometheus = pkgs.writeShellApplication {
        name = "ns-prometheus";
        runtimeInputs = [ monitoringPkgs.prometheus ];
        text = ''
          mkdir -p data/prometheus
          exec prometheus \
            --config.file=${prometheusConfig} \
            --storage.tsdb.path=data/prometheus \
            --storage.tsdb.retention.time=30d \
            --web.listen-address=127.0.0.1:9090
        '';
      };

      redisExporter = pkgs.writeShellApplication {
        name = "ns-redis-exporter";
        runtimeInputs = [ monitoringPkgs.prometheus-redis-exporter ];
        text = ''
          exec redis_exporter \
            --web.listen-address=127.0.0.1:9121
        '';
      };

      grafana = pkgs.writeShellApplication {
        name = "ns-grafana";
        runtimeInputs = [ monitoringPkgs.grafana ];
        text = ''
          root="$PWD"
          provisioning="data/grafana/provisioning"
          mkdir -p "$provisioning/datasources" "$provisioning/dashboards" data/grafana/data

          ln -sfn ${grafanaDatasource} "$provisioning/datasources/datasource.yaml"
          printf 'apiVersion: 1\nproviders:\n  - name: bench\n    folder: Notification Service\n    type: file\n    allowUiUpdates: false\n    options:\n      path: %s/k8s/dashboards\n' \
            "$root" > "$provisioning/dashboards/dashboards.yaml"

          mkdir -p data/grafana/logs data/grafana/plugins data/grafana/data/png

          export GF_PATHS_DATA="$root/data/grafana/data"
          export GF_PATHS_LOGS="$root/data/grafana/logs"
          export GF_PATHS_PLUGINS="$root/data/grafana/plugins"
          export GF_PATHS_PROVISIONING="$root/$provisioning"
          export GF_SERVER_HTTP_ADDR=127.0.0.1
          export GF_SERVER_HTTP_PORT=3000
          export GF_AUTH_ANONYMOUS_ENABLED=true
          export GF_AUTH_ANONYMOUS_ORG_ROLE=Admin
          export GF_AUTH_DISABLE_LOGIN_FORM=true
          export GF_USERS_DEFAULT_THEME=dark
          export GF_ANALYTICS_REPORTING_ENABLED=false

          exec grafana server --homepath ${monitoringPkgs.grafana}/share/grafana
        '';
      };
    in
    {
      process-compose."notification-services" = {
        imports = [
          inputs.services-flake.processComposeModules.default
        ];
        services.redis-cluster."redis" = {
          enable = true;
        };
        settings.processes = {
          prometheus.command = lib.getExe prometheus;
          grafana.command = lib.getExe grafana;
          redis-exporter.command = lib.getExe redisExporter;
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
