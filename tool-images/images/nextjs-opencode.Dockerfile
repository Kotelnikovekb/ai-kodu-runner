# Copyright 2026 Kotelnikovekb
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://apache.org
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
FROM aquasec/trivy:0.74.0 AS trivy

FROM node:22-bookworm

LABEL org.opencontainers.image.title="Next.js OpenCode tool image" \
    org.opencontainers.image.licenses="Apache-2.0" \
    org.opencontainers.image.source="https://github.com/KotelnikoffDev/ai-kodu-runner"

ENV HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_CACHE_HOME=/home/opencode/.cache \
    XDG_STATE_HOME=/home/opencode/.local/state \
    NPM_CONFIG_CACHE=/home/opencode/.cache/npm \
    TRIVY_CACHE_DIR=/home/opencode/.cache/trivy \
    OPENCODE_DB=:memory: \
    NPM_CONFIG_UPDATE_NOTIFIER=false \
    NEXT_TELEMETRY_DISABLED=1

RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends ca-certificates curl git bash build-essential \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global opencode-ai@latest npm@latest \
    && npm cache clean --force \
    && useradd --create-home --home-dir /home/opencode --shell /bin/bash opencode \
    && mkdir -p /home/opencode/.config /home/opencode/.local/share \
        /home/opencode/.local/state /home/opencode/.cache/npm /home/opencode/.cache/trivy /workspace \
    && chown -R opencode:opencode /home/opencode /workspace

COPY --from=trivy /usr/local/bin/trivy /usr/local/bin/trivy

RUN node --version && npm --version && opencode --version && trivy --version

WORKDIR /workspace
USER opencode
ENTRYPOINT []
CMD ["bash"]
