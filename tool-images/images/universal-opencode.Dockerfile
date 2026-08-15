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

ARG OPENCODE_VERSION=1.18.18

LABEL org.opencontainers.image.title="Universal OpenCode tool image" \
    org.opencontainers.image.description="Headless OpenCode image with common CLI tools" \
    org.opencontainers.image.version="${OPENCODE_VERSION}" \
    org.opencontainers.image.licenses="Apache-2.0" \
    org.opencontainers.image.source="https://github.com/KotelnikoffDev/ai-kodu-runner"

ENV HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_CACHE_HOME=/home/opencode/.cache \
    XDG_STATE_HOME=/home/opencode/.local/state \
    NPM_CONFIG_CACHE=/home/opencode/.cache/npm \
    CI=true \
    OPENCODE_DB=:memory: \
    OPENCODE_DISABLE_AUTOUPDATE=true \
    OPENCODE_EXPERIMENTAL_LSP_TOOL=true \
    PATH="/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

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
    && mkdir -p /home/opencode/.config/opencode \
        /home/opencode/.local/share/opencode \
        /home/opencode/.local/state \
        /home/opencode/.cache/opencode \
        /home/opencode/.cache/npm \
        /workspace \
    && chown -R opencode:opencode /home/opencode /workspace \
    && rm -rf /var/lib/apt/lists/*

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN curl -fsSL https://opencode.ai/install \
        | bash -s -- --version "${OPENCODE_VERSION}" --no-modify-path \
    && install -m 0755 /home/opencode/.opencode/bin/opencode /usr/local/bin/opencode \
    && rm -rf /home/opencode/.opencode \
    && test "$(opencode --version)" = "${OPENCODE_VERSION}"

COPY --chmod=0755 runner-entrypoint.sh /usr/local/bin/runner-entrypoint

WORKDIR /workspace
USER 10001:10001

RUN bash -lc 'test "$(id -u)" = 10001 \
    && test "$(command -v opencode)" = /usr/local/bin/opencode \
    && command -v git \
    && command -v rg \
    && command -v fd \
    && opencode --version'

ENTRYPOINT ["runner-entrypoint"]
CMD ["bash"]
