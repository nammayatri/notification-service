let redis_cfg = {
    redis_host = "0.0.0.0",
    redis_port = 6380,
    redis_pool_size = 10,
    redis_partition = 0,
    reconnect_max_attempts = 10,
    reconnect_delay = 5000,
    default_ttl = 3600,
    default_hash_ttl = 3600,
    stream_read_count = 100,
}

let LogLevel = < TRACE | DEBUG | INFO | WARN | ERROR | OFF >

let logger_cfg = {
    level = LogLevel.INFO,
    log_to_file = False
}

in {
    logger_cfg = logger_cfg,
    redis_cfg = redis_cfg,
    port = 50051
}