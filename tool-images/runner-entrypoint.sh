#!/usr/bin/env bash
# Copyright 2026 Kotelnikovekb
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

# The runner overlays these paths with empty writable tmpfs mounts when the
# container root filesystem is read-only. Keep the runtime contract explicit
# and fail early if an image or executor no longer provides a writable path.
runtime_dirs=(
    "${XDG_CONFIG_HOME:-/home/opencode/.config}/opencode"
    "${XDG_DATA_HOME:-/home/opencode/.local/share}/opencode"
    "${XDG_STATE_HOME:-/home/opencode/.local/state}"
    "${XDG_CACHE_HOME:-/home/opencode/.cache}/opencode"
    "${NPM_CONFIG_CACHE:-/home/opencode/.cache/npm}"
)

for runtime_dir in "${runtime_dirs[@]}"; do
    mkdir -p "${runtime_dir}"
done

# Flutter's SDK dependencies are prepared at image-build time. PUB_CACHE is an
# empty tmpfs at runtime, so seed it without mutating the read-only Flutter SDK.
pub_seed=/opt/flutter-pub-cache-seed
if [[ -d "${pub_seed}" ]]; then
    pub_cache="${PUB_CACHE:-/home/opencode/.pub-cache}"
    if [[ ! -e "${pub_cache}/.runner-seeded" ]]; then
        mkdir -p "${pub_cache}"
        cp -a "${pub_seed}/." "${pub_cache}/"
        touch "${pub_cache}/.runner-seeded"
    fi
fi

exec "$@"
