#!/usr/bin/env bash
set -euo pipefail

run_cobi() {
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local bin_path="${script_dir}/target/release/cobi"
    local pid_file="${script_dir}/cobi.pid"

    local default_src="SQL_sample_DB"
    local default_dst="${script_dir}/output_sqlite"

    local src_dir="${1:-$default_src}"
    local dst_dir="${2:-$default_dst}"

    if [[ ! -x "$bin_path" ]]; then
        echo "Building cobi in release mode..."
        (cd "$script_dir" && make release)
    fi

    if [[ $# -ge 2 ]]; then
        shift 2
    elif [[ $# -eq 1 ]]; then
        shift 1
    fi

    echo "Starting Cobi SQL Server (.mdf) -> SQLite3 migration..."
    echo "  Source:      $src_dir"
    echo "  Destination: $dst_dir"

    "$bin_path" "$src_dir" "$dst_dir" -j 4 "$@" &
    local pid=$!
    echo "$pid" > "$pid_file"
    wait "$pid"
    rm -f "$pid_file"
}

run_cobi "$@"
