FROM ghcr.io/cirruslabs/flutter:stable

USER root

ENV HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_CACHE_HOME=/home/opencode/.cache \
    XDG_STATE_HOME=/home/opencode/.local/state \
    NPM_CONFIG_CACHE=/home/opencode/.cache/npm \
    PUB_CACHE=/home/opencode/.cache/pub \
    OPENCODE_DB=:memory: \
    PATH="/home/opencode/.opencode/bin:${PATH}" \
    OPENCODE_EXPERIMENTAL_LSP_TOOL=true

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        git \
        bash \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /home/opencode/.config /home/opencode/.local/share \
        /home/opencode/.local/state /home/opencode/.cache/npm /home/opencode/.cache/pub

RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && npm --version \
    && node --version \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://opencode.ai/install | bash \
    && ln -s /home/opencode/.opencode/bin/opencode /usr/local/bin/opencode \
    && opencode --version

WORKDIR /workspace

ENTRYPOINT []
CMD ["sh"]
