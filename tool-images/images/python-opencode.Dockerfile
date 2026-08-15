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
FROM python:3.12-slim

LABEL org.opencontainers.image.title="Python OpenCode tool image" \
    org.opencontainers.image.licenses="Apache-2.0" \
    org.opencontainers.image.source="https://github.com/KotelnikoffDev/ai-kodu-runner"

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_STATE_HOME=/home/opencode/.local/state \
    OPENCODE_DB=:memory: \
    PATH="/home/opencode/.local/bin:/home/opencode/.opencode/bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    PIP_CACHE_DIR=/home/opencode/.cache/pip

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git nodejs npm passwd \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash opencode \
    && mkdir -p /home/opencode/.opencode /home/opencode/.config \
        /home/opencode/.local/share /home/opencode/.local/state /home/opencode/.cache/pip /workspace \
    && chown -R opencode:opencode /home/opencode /workspace

USER opencode
RUN curl -fsSL https://opencode.ai/install | bash

USER root
RUN ln -sf /home/opencode/.opencode/bin/opencode /usr/local/bin/opencode
USER opencode

WORKDIR /workspace
ENTRYPOINT []
CMD ["bash"]
