FROM ghcr.io/cirruslabs/flutter:stable

USER root

ENV HOME=/home/opencode
ENV PATH="/home/opencode/.opencode/bin:${PATH}"
ENV OPENCODE_EXPERIMENTAL_LSP_TOOL=true

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl \
        ca-certificates \
        git \
        bash \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /home/opencode

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
