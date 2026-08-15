#!/usr/bin/env bash
set -euo pipefail

# Runner mounts PUB_CACHE as an empty writable tmpfs when rootfs is read-only.
# Seed it with the Flutter tool dependencies prepared while building the image,
# otherwise Flutter tries to rewrite its own SDK lockfiles at job startup.
pub_cache="${PUB_CACHE:-/home/opencode/.pub-cache}"
if [[ ! -e "${pub_cache}/.runner-seeded" ]]; then
    mkdir -p "${pub_cache}"
    cp -a /opt/flutter-pub-cache-seed/. "${pub_cache}/"
    touch "${pub_cache}/.runner-seeded"
fi

exec "$@"
