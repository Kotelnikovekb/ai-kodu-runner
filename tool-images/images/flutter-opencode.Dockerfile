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
ARG android_sdk_ver=36
FROM ghcr.io/cirruslabs/android-sdk:${android_sdk_ver}

ARG cmake_version=3.22.1
ARG ndk_version=28.2.13676358
ARG flutter_ver=3.41.6
ARG image_revision=4

LABEL org.opencontainers.image.title="Flutter OpenCode" \
    org.opencontainers.image.description="Flutter and Android SDK tool image with OpenCode" \
    org.opencontainers.image.version="${image_revision}" \
    org.opencontainers.image.licenses="Apache-2.0" \
    org.opencontainers.image.source="https://github.com/KotelnikoffDev/ai-kodu-runner"

ENV FLUTTER_HOME=/usr/local/flutter \
    HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_CACHE_HOME=/home/opencode/.cache \
    XDG_STATE_HOME=/home/opencode/.local/state \
    NPM_CONFIG_CACHE=/home/opencode/.cache/npm \
    PUB_CACHE=/home/opencode/.pub-cache \
    CI=true \
    OPENCODE_DB=:memory: \
    OPENCODE_EXPERIMENTAL_LSP_TOOL=true \
    ANDROID_NDK_HOME=${ANDROID_SDK_ROOT}/ndk/${ndk_version} \
    PATH=/home/opencode/.opencode/bin:/usr/local/flutter/bin:/usr/local/flutter/bin/cache/dart-sdk/bin:${PATH}

# hadolint ignore=DL3008
# The runner mounts these directories as ephemeral tmpfs volumes when it uses
# a read-only root filesystem. Keep the mount points in the image as well.
RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends --no-install-suggests \
        bash \
        build-essential \
        ca-certificates \
        clang \
        curl \
        git \
        lcov \
        libgtk-3-dev \
        liblzma-dev \
        ninja-build \
        pkg-config \
        unzip \
        xz-utils \
    && update-ca-certificates \
    && useradd --create-home --uid 10001 --user-group --shell /bin/bash opencode \
    && mkdir -p /home/opencode/.config/opencode \
        /home/opencode/.local/share/opencode/log \
        /home/opencode/.local/state /home/opencode/.cache/npm \
        /home/opencode/.pub-cache /workspace \
    && chown -R opencode:opencode /home/opencode /workspace \
    && rm -rf /var/lib/apt/lists/*

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN (set +o pipefail; yes | sdkmanager --licenses >/dev/null) \
    && sdkmanager \
        "platform-tools" \
        "platforms;android-33" \
        "platforms;android-34" \
        "platforms;android-35" \
        "build-tools;35.0.0" \
        "ndk;${ndk_version}" \
        "cmake;${cmake_version}"

RUN git clone --depth 1 --branch "${flutter_ver}" \
        https://github.com/flutter/flutter.git "${FLUTTER_HOME}" \
    && git config --global --add safe.directory /usr/local/flutter

RUN flutter config --enable-android --enable-linux-desktop --enable-web --no-enable-ios \
    && flutter precache --universal --linux --web --no-ios \
    && (set +o pipefail; yes | flutter doctor --android-licenses) \
    && flutter --version \
    && chown -R 10001:10001 /usr/local/flutter /home/opencode

USER 10001:10001

RUN flutter config --no-analytics \
    && flutter --version \
    && dart --version

RUN curl -fsSL https://opencode.ai/install | bash \
    && opencode --version

WORKDIR /workspace
ENTRYPOINT []
CMD ["bash"]
