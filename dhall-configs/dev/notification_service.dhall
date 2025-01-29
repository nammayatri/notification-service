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
    level = LogLevel.WARN,
    log_to_file = False
}

let driver_internal_auth_config = {
    auth_url = "http://127.0.0.1:8016/internal/auth",
    auth_api_key = "ae288466-2add-11ee-be56-0242ac120002",
    auth_token_expiry = 86400
}

let driver_dashboard_internal_auth_config = {
    auth_url = "http://127.0.0.1:8016/internal/auth",
    auth_api_key = "ae288466-2add-11ee-be56-0242ac120002",
    auth_token_expiry = 86400
}

let tokenOriginInternalAuthMap =
    { DriverApp = driver_internal_auth_config
    , RiderApp = driver_internal_auth_config
    , DriverDashboard = driver_dashboard_internal_auth_config
    , RiderDashboard = driver_dashboard_internal_auth_config
    }

in {
    grpc_port = 50051,
    http_server_port = 9091,
    internal_auth_cfg = tokenOriginInternalAuthMap,
    logger_cfg = logger_cfg,
    redis_cfg = redis_cfg,
    reader_delay_millis = 100,
    max_shards = +5,
    channel_buffer = 100000,
    redis_retry_bucket_expiry = 7210,
    redis_retry_key_window = 3600,
    request_timeout_seconds = 60
}