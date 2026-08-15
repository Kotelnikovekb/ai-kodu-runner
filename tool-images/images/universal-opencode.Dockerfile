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
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="Universal OpenCode tool image" \
    org.opencontainers.image.licenses="Apache-2.0" \
    org.opencontainers.image.source="https://github.com/KotelnikoffDev/ai-kodu-runner"

ENV HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_CACHE_HOME=/home/opencode/.cache \
    XDG_STATE_HOME=/home/opencode/.local/state \
    OPENCODE_DB=:memory: \
    PATH=/home/opencode/.local/bin:/home/opencode/.opencode/bin:${PATH}

# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        fd-find \
        git \
        jq \
        less \
        openssh-client \
        procps \
        ripgrep \
        unzip \
        zip \
    && ln -s /usr/bin/fdfind /usr/local/bin/fd \
    && useradd --create-home --uid 10001 --user-group --shell /bin/bash opencode \
    && mkdir -p /home/opencode/.config /home/opencode/.local/share \
        /home/opencode/.local/state /home/opencode/.cache /workspace \
    && chown -R opencode:opencode /home/opencode /workspace \
    && rm -rf /var/lib/apt/lists/*

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

USER 10001:10001
RUN curl -fsSL https://opencode.ai/install | bash \
    && opencode --version

WORKDIR /workspace
ENTRYPOINT []
CMD ["bash"]
