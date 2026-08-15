#!/bin/sh
# Copyright 2026 Kotelnikovekb
# SPDX-License-Identifier: Apache-2.0

set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <image> <flutter|nextjs|python|php|universal>" >&2
    exit 2
fi

image=$1
kind=$2

case "${kind}" in
    flutter)
        tools='flutter dart node npm opencode trivy'
        ;;
    nextjs)
        tools='node npm opencode trivy'
        ;;
    python)
        tools='python pip node npm opencode'
        ;;
    php)
        tools='php composer node npm opencode'
        ;;
    universal)
        tools='git rg fd jq opencode'
        ;;
    *)
        echo "unsupported image kind: ${kind}" >&2
        exit 2
        ;;
esac

docker run --rm \
    --read-only \
    --network none \
    --tmpfs /tmp:rw,nosuid,nodev,uid=10001,gid=10001,mode=1777,size=1g \
    --tmpfs /workspace:rw,nosuid,nodev,uid=10001,gid=10001,mode=0755,size=1g \
    --tmpfs /home/opencode/.config/opencode:rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=512m \
    --tmpfs /home/opencode/.local/share/opencode:rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=1g \
    --tmpfs /home/opencode/.local/state:rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=128m \
    --tmpfs /home/opencode/.cache/opencode:rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=128m \
    --tmpfs /home/opencode/.cache/npm:rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=1g \
    --tmpfs /home/opencode/.pub-cache:rw,nosuid,nodev,uid=10001,gid=10001,mode=0700,size=1g \
    "${image}" \
    bash -lc '
        set -euo pipefail
        test "$(id -u)" = 10001
        test "$(pwd)" = /workspace
        for runtime_dir in \
            "$XDG_CONFIG_HOME/opencode" \
            "$XDG_DATA_HOME/opencode" \
            "$XDG_STATE_HOME" \
            "$XDG_CACHE_HOME/opencode" \
            "$NPM_CONFIG_CACHE"
        do
            touch "$runtime_dir/.runner-smoke"
            rm "$runtime_dir/.runner-smoke"
        done
        for tool in '"${tools}"'
        do
            command -v "$tool"
        done
        opencode --version
    '
