let redis_cfg = {
    host = "0.0.0.0",
    port = 30001,
    cluster_enabled = True,
    cluster_urls = [""],
    use_legacy_version = False,
    pool_size = 50,
    reconnect_max_attempts = 10,
    reconnect_delay = 5000,
    default_ttl = 3600,
    default_hash_ttl = 3600,
    stream_read_count = 100,
    partition = 0,
}

let LogLevel = < TRACE | DEBUG | INFO | WARN | ERROR | OFF >

let logger_cfg = {
    level = LogLevel.DEBUG,
    log_to_file = False
}

let kafka_cfg = {
    kafka_key = "bootstrap.servers",
    kafka_host = "0.0.0.0:29092"
}

let internal_auth_cfg = {
    auth_url = "http://127.0.0.1:8016/internal/auth",
    auth_api_key = "ae288466-2add-11ee-be56-0242ac120002",
    auth_token_expiry = 86400
}

in {
    grpc_port = 50051,
    http_server_port = 9090,
    internal_auth_cfg = internal_auth_cfg,
    logger_cfg = logger_cfg,
    redis_cfg = redis_cfg,
    kafka_cfg = kafka_cfg,
    last_known_notification_cache_expiry = 86400,
    notification_kafka_topic = "notification-stats",
    reader_delay_seconds = 1,
    retry_delay_seconds = 5,
    max_shards = +15,
}