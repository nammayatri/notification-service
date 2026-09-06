## List all client streams
# ./scripts/send-notification.sh -l
# Send a notification to all clients
# ./scripts/send-notification.sh "Overspeeding Alert" "Bus KA01AB1234 crossed 80 km/h"

#!/usr/bin/env bash
set -euo pipefail

REDIS_HOST=${REDIS_HOST:-127.0.0.1}
REDIS_PORT=${REDIS_PORT:-30001}

VALID_CATEGORIES="WMB_ALERT ONBOARDING_UPDATE FLEET_UPDATE GENERIC_NOTIFICATION"
VALID_ENTITY_TYPES="DriverEntity VehicleEntity FleetEntity TripTransactionEntity RideEntity"
VALID_PLATFORMS="DRIVER RIDER"

CATEGORY="GENERIC_NOTIFICATION"
ENTITY_TYPE="DriverEntity"
ENTITY_ID=""
PLATFORM="DRIVER"
DATA=""
SHOW="SHOW"
TTL_SECONDS=3600
CLIENT_FILTER=""
CLIENT_ID=""
MONITOR_SECONDS=${MONITOR_SECONDS:-2}
LIST_ONLY=0
DRY_RUN=0

DHALL_CONFIG=${DHALL_CONFIG:-$(dirname "$0")/../dhall-configs/dev/notification_service.dhall}
MAX_SHARDS=${MAX_SHARDS:-$(sed -n 's/.*max_shards *= *+\([0-9]*\).*/\1/p' "$DHALL_CONFIG" 2>/dev/null | head -1)}
MAX_SHARDS=${MAX_SHARDS:-5}

usage() {
    cat <<'USAGE'
Usage: send-notification.sh [options] <title> <body>

Fans a notification out to every connected client stream found in Redis.

Options:
  -c <category>     WMB_ALERT | ONBOARDING_UPDATE | FLEET_UPDATE | GENERIC_NOTIFICATION
  -t <entityType>   DriverEntity | VehicleEntity | FleetEntity | TripTransactionEntity | RideEntity
  -e <entityId>     entity.id (default: a fresh uuid)
  -p <platform>     DRIVER | RIDER (default: DRIVER)
  -d <json>         entity.data JSON object (platform is injected if absent)
  -s <show>         show flag (default: SHOW)
  -x <seconds>      ttl from now (default: 3600)
  -k <substring>    only clients whose id contains this substring
  -C <clientId>     target this client id explicitly, shard derived from hash_uuid
  -w <seconds>      MONITOR sampling window that catches idle clients (default 2, 0 disables)
  -l                list matching client streams and exit
  -n                dry run, print the XADD commands only
  -h                this help

Clients are discovered from the keyspace (streams that still hold entries) and
from a short MONITOR sample of the reader XREAD polling (clients whose stream
is currently empty).

Env: REDIS_HOST (default 127.0.0.1), REDIS_PORT (default 30001),
     MAX_SHARDS (default read from dhall-configs/dev/notification_service.dhall)
USAGE
}

while getopts ":c:t:e:p:d:s:x:k:C:w:lnh" opt; do
    case "$opt" in
        c) CATEGORY=$OPTARG ;;
        t) ENTITY_TYPE=$OPTARG ;;
        e) ENTITY_ID=$OPTARG ;;
        p) PLATFORM=$OPTARG ;;
        d) DATA=$OPTARG ;;
        s) SHOW=$OPTARG ;;
        x) TTL_SECONDS=$OPTARG ;;
        k) CLIENT_FILTER=$OPTARG ;;
        C) CLIENT_ID=$OPTARG ;;
        w) MONITOR_SECONDS=$OPTARG ;;
        l) LIST_ONLY=1 ;;
        n) DRY_RUN=1 ;;
        h) usage; exit 0 ;;
        *) usage >&2; exit 1 ;;
    esac
done
shift $((OPTIND - 1))

contains_word() {
    case " $1 " in *" $2 "*) return 0 ;; *) return 1 ;; esac
}

if [ "$LIST_ONLY" -eq 0 ]; then
    if [ $# -lt 2 ]; then
        usage >&2
        exit 1
    fi
    contains_word "$VALID_CATEGORIES" "$CATEGORY" || { echo "invalid category: $CATEGORY" >&2; exit 1; }
    contains_word "$VALID_ENTITY_TYPES" "$ENTITY_TYPE" || { echo "invalid entity type: $ENTITY_TYPE" >&2; exit 1; }
    contains_word "$VALID_PLATFORMS" "$PLATFORM" || { echo "invalid platform: $PLATFORM" >&2; exit 1; }
fi

TITLE=${1:-}
BODY=${2:-}

uuid() {
    if command -v uuidgen >/dev/null 2>&1; then
        uuidgen | tr '[:upper:]' '[:lower:]'
    else
        python3 -c 'import uuid; print(uuid.uuid4())'
    fi
}

iso_now() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

iso_in() {
    if date -u -v+"$1"S +%Y-%m-%dT%H:%M:%SZ >/dev/null 2>&1; then
        date -u -v+"$1"S +%Y-%m-%dT%H:%M:%SZ
    else
        date -u -d "+$1 seconds" +%Y-%m-%dT%H:%M:%SZ
    fi
}

cluster_nodes() {
    if redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" CLUSTER INFO 2>/dev/null | grep -q 'cluster_enabled:1'; then
        redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" CLUSTER NODES \
            | awk '$3 ~ /master/ { split($2, a, "@"); print a[1] }'
    else
        echo "$REDIS_HOST:$REDIS_PORT"
    fi
}

keyspace_streams() {
    for node in $(cluster_nodes); do
        redis-cli -h "${node%%:*}" -p "${node##*:}" --scan --pattern 'N*{*}' 2>/dev/null
    done | grep -E '^N[0-9a-fA-F-]{36}\{[0-9]+\}$' || true
}

monitored_streams() {
    [ "$MONITOR_SECONDS" -gt 0 ] 2>/dev/null || return 0
    local sample pids=()
    sample=$(mktemp)
    for node in $(cluster_nodes); do
        redis-cli -h "${node%%:*}" -p "${node##*:}" MONITOR >>"$sample" 2>/dev/null &
        pids+=($!)
    done
    sleep "$MONITOR_SECONDS"
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait "${pids[@]}" 2>/dev/null || true
    grep -oE 'N[0-9a-fA-F-]{36}\{[0-9]+\}' "$sample" || true
    rm -f "$sample"
}

stream_key_for() {
    local shard
    shard=$(python3 -c 'import sys, uuid; raw = uuid.UUID(sys.argv[1]).bytes; print((int.from_bytes(raw[:8], "big") + int.from_bytes(raw[8:], "big")) % int(sys.argv[2]))' "$1" "$MAX_SHARDS") \
        || { echo "invalid client id: $1" >&2; exit 1; }
    printf 'N%s{%s}' "$1" "$shard"
}

client_streams() {
    { keyspace_streams; monitored_streams; } | sort -u
}

build_data() {
    if [ -z "$DATA" ]; then
        printf '{"platform":"%s"}' "$PLATFORM"
    elif printf '%s' "$DATA" | grep -q '"platform"'; then
        printf '%s' "$DATA"
    else
        printf '{"platform":"%s",%s' "$PLATFORM" "${DATA#\{}"
    fi
}

if [ -n "$CLIENT_ID" ]; then
    STREAMS=$(stream_key_for "$CLIENT_ID")
else
    STREAMS=$(client_streams)
fi
if [ -n "$CLIENT_FILTER" ]; then
    STREAMS=$(printf '%s\n' "$STREAMS" | grep -F "$CLIENT_FILTER" || true)
fi

if [ -z "$STREAMS" ]; then
    echo "no client streams found on $REDIS_HOST:$REDIS_PORT (is a client connected?)" >&2
    exit 1
fi

if [ "$LIST_ONLY" -eq 1 ]; then
    printf '%s\n' "$STREAMS"
    exit 0
fi

CREATED_AT=$(iso_now)
TTL_AT=$(iso_in "$TTL_SECONDS")
ENTITY_DATA=$(build_data)

sent=0
while IFS= read -r key; do
    [ -n "$key" ] || continue
    client_id=${key%%\{*}
    client_id=${client_id#N}
    shard=${key##*\{}
    shard=${shard%\}}
    entity_id=${ENTITY_ID:-$(uuid)}

    args=(
        XADD "$key" '*'
        id "$(uuid)"
        category "$CATEGORY"
        title "$TITLE"
        body "$BODY"
        show "$SHOW"
        created_at "$CREATED_AT"
        ttl "$TTL_AT"
        entity.id "$entity_id"
        entity.type "$ENTITY_TYPE"
        entity.data "$ENTITY_DATA"
    )

    if [ "$DRY_RUN" -eq 1 ]; then
        printf 'redis-cli -c -h %s -p %s' "$REDIS_HOST" "$REDIS_PORT"
        printf ' %q' "${args[@]}"
        printf '\n'
    else
        stream_id=$(redis-cli -c -h "$REDIS_HOST" -p "$REDIS_PORT" "${args[@]}")
        echo "client=$client_id shard=$shard stream_id=$stream_id"
    fi
    sent=$((sent + 1))
done <<< "$STREAMS"

echo "$sent notification(s) $([ "$DRY_RUN" -eq 1 ] && echo planned || echo queued)"
