let redis_cfg = {
    host = "redis.fdmlwb.clustercfg.aps1.cache.amazonaws.com",
    port = 6379,
    cluster_enabled = True,
    cluster_urls = ["redis.fdmlwb.clustercfg.aps1.cache.amazonaws.com:6379"],
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

let driver_internal_auth_config = {
    auth_url = "https://api.sandbox.moving.tech/dev/dobpp/internal/auth",
    auth_api_key = "ae288466-2add-11ee-be56-0242ac120002",
    auth_token_expiry = 86400
}


let driver_dashboard_internal_auth_config = {
    auth_url = "https://dashboard.integ.moving.tech/api/dev/bpp/driver-offer/internal/auth",
    auth_api_key = "ae288466-2add-11ee-be56-0242ac120002",
    auth_token_expiry = 86400
}

let internal_auth_cfg =
    { DriverApp = driver_internal_auth_config
    , RiderApp = driver_internal_auth_config
    , DriverDashboard = driver_dashboard_internal_auth_config
    , RiderDashboard = driver_dashboard_internal_auth_config
    }

in {
    grpc_port = 50051,
    http_server_port = 9090,
    internal_auth_cfg = internal_auth_cfg,
    logger_cfg = logger_cfg,
    redis_cfg = redis_cfg,
    last_known_notification_cache_expiry = 86400,
    retry_delay_millis = 3600000,
    max_shards=128,
    channel_buffer = 100000,
    request_timeout_seconds = 31536000, -- 1 Year
    read_all_connected_client_notifications = False
}