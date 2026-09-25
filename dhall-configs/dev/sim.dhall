let AckMode = < Instant | Delayed | Distribution | Never >

let ReconnectMode = < ServerOnly | Distribution >

let LifetimeBucket = { upto_seconds : Double, cumulative_count : Double }

let prod_stream_lifetime_buckets
    : List LifetimeBucket
    = [ { upto_seconds = 0.005, cumulative_count = 3.964 }
      , { upto_seconds = 0.01, cumulative_count = 4.229 }
      , { upto_seconds = 0.025, cumulative_count = 4.450 }
      , { upto_seconds = 0.05, cumulative_count = 4.626 }
      , { upto_seconds = 0.1, cumulative_count = 4.998 }
      , { upto_seconds = 0.25, cumulative_count = 5.641 }
      , { upto_seconds = 0.5, cumulative_count = 6.165 }
      , { upto_seconds = 1.0, cumulative_count = 6.544 }
      , { upto_seconds = 2.5, cumulative_count = 6.767 }
      , { upto_seconds = 10.0, cumulative_count = 6.856 }
      , { upto_seconds = 20.0, cumulative_count = 6.901 }
      , { upto_seconds = 50.0, cumulative_count = 6.975 }
      , { upto_seconds = 100.0, cumulative_count = 7.099 }
      , { upto_seconds = 200.0, cumulative_count = 7.281 }
      , { upto_seconds = 300.0, cumulative_count = 7.831 }
      , { upto_seconds = 400.0, cumulative_count = 8.386 }
      ]

let healthy_stream_lifetime_buckets
    : List LifetimeBucket
    = [ { upto_seconds = 1.0, cumulative_count = 0.2 }
      , { upto_seconds = 10.0, cumulative_count = 0.8 }
      , { upto_seconds = 60.0, cumulative_count = 2.0 }
      , { upto_seconds = 300.0, cumulative_count = 3.0 }
      , { upto_seconds = 3600.0, cumulative_count = 100.0 }
      ]

let clients =
      { endpoints = [ "http://127.0.0.1:50051" ]
      , client_index_start = 0
      , client_count = 100
      , ack_mode = AckMode.Distribution
      , ack_delay_ms = 0
      , ack_p50_ms = 800
      , ack_p90_ms = 2071
      , ack_p95_ms = 2326
      , ack_max_ms = 5000
      , reconnect_mode = ReconnectMode.Distribution
      , reconnect_lifetime_buckets = prod_stream_lifetime_buckets
      , never_ack_pct = 1
      , metrics_port = 9101
      , max_shards = 128
      , streams_per_connection = 1
      , connect_rate_per_sec = 500.0
      , token_origin = "DriverApp"
      }

let producer =
      { target_rate = 50.0
      , client_count = 100
      , fleet_client_count = 100
      , client_index_start = 0
      , fanout = 20
      , burst = True
      , publish_enabled = True
      , ttl_seconds = 30
      , payload_bytes = 1400
      , metrics_port = 9102
      , max_shards = 128
      , stream_expiration_seconds = 3600
      , max_inflight_search_requests = 512
      }

in  { clients, producer }
