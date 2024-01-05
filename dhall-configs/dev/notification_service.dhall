let redis_cfg = {
    host = "0.0.0.0",
    port = 30001,
    cluster_enabled = True,
    cluster_urls = [],
    use_legacy_version = False,
    pool_size = 10,
    reconnect_max_attempts = 10,
    reconnect_delay = 5000,
    default_ttl = 3600,
    default_hash_ttl = 3600,
    stream_read_count = 100,
    partition = 0,
}

let LogLevel = < TRACE | DEBUG | INFO | WARN | ERROR | OFF >

let logger_cfg = {
    level = LogLevel.INFO,
    log_to_file = False
}

let kafka_cfg = {
    kafka_key = "bootstrap.servers",
    kafka_host = "0.0.0.0:29092"
}

in {
    port = 50051,
    prometheus_port = 9090,
    logger_cfg = logger_cfg,
    redis_cfg = redis_cfg,
    kafka_cfg = kafka_cfg,
    reader_delay_seconds = 1,
    retry_delay_seconds = 60,
}