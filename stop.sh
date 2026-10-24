#!/usr/bin/env bash
set -euo pipefail

stop_cobi() {
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local pid_file="${script_dir}/cobi.pid"

    if [[ -f "$pid_file" ]]; then
        local pid
        pid="$(cat "$pid_file")"
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            echo "Stopping cobi process (PID: $pid)..."
            kill "$pid"
        else
            echo "Process $pid is not running."
        fi
        rm -f "$pid_file"
    else
        echo "No cobi.pid file found."
    fi
}

stop_cobi "$@"
