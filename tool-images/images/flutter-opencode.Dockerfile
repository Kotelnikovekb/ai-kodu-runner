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
ARG trivy_version=0.74.0
ARG android_sdk_ver=36

FROM aquasec/trivy:${trivy_version} AS trivy

FROM ghcr.io/cirruslabs/android-sdk:${android_sdk_ver}

ARG cmake_version=3.22.1
ARG ndk_version=28.2.13676358
ARG flutter_ver=3.41.6
ARG opencode_version=1.18.18
ARG image_revision=7

LABEL org.opencontainers.image.title="Flutter OpenCode" \
    org.opencontainers.image.description="Flutter, Android SDK, Node.js, Trivy, and headless OpenCode tool image" \
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
    TRIVY_CACHE_DIR=/workspace/.cache/trivy \
    CI=true \
    OPENCODE_DB=:memory: \
    OPENCODE_DISABLE_AUTOUPDATE=true \
    OPENCODE_EXPERIMENTAL_LSP_TOOL=true \
    ANDROID_NDK_HOME=${ANDROID_SDK_ROOT}/ndk/${ndk_version} \
    PATH=/home/opencode/.opencode/bin:/usr/local/flutter/bin:/usr/local/flutter/bin/cache/dart-sdk/bin:${PATH}

# The runner mounts these directories as ephemeral tmpfs volumes when it uses
# a read-only root filesystem. Keep the mount points in the image as well.
# hadolint ignore=DL3008
RUN apt-get update \
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
        /home/opencode/.local/share/opencode \
        /home/opencode/.local/state \
        /home/opencode/.cache/opencode \
        /home/opencode/.cache/npm \
        /home/opencode/.pub-cache \
        /workspace \
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
        https://github.com/flutter/flutter.git "${FLUTTER_HOME}"

RUN git config --system --add safe.directory "${FLUTTER_HOME}" \
    && flutter config --enable-android --enable-linux-desktop --enable-web \
    && flutter precache --android --ios --universal --force \
    && (set +o pipefail; yes | flutter doctor --android-licenses) \
    && flutter_path="$(readlink -f "$(command -v flutter)")" \
    && dart_path="$(readlink -f "$(command -v dart)")" \
    && ln -sf "${flutter_path}" /usr/local/bin/flutter \
    && ln -sf "${dart_path}" /usr/local/bin/dart

RUN flutter config --no-analytics \
    && flutter --version \
    && dart --version

RUN curl -fsSL https://opencode.ai/install \
        | bash -s -- --version "${opencode_version}" --no-modify-path \
    && install -m 0755 /home/opencode/.opencode/bin/opencode /usr/local/bin/opencode \
    && rm -rf /home/opencode/.opencode \
    && test "$(opencode --version)" = "${opencode_version}"

# hadolint ignore=DL3008
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

COPY --from=trivy /usr/local/bin/trivy /usr/local/bin/trivy
COPY --chmod=0755 runner-entrypoint.sh /usr/local/bin/runner-entrypoint

# Flutter 3.41 materializes its final engine stamp on the first non-root tool
# invocation. Permit that only while building; the SDK is returned to root
# ownership after the smoke project has been analyzed and tested.
RUN chown -R 10001:10001 /home/opencode "${FLUTTER_HOME}"

WORKDIR /workspace
ENV FLUTTER_ALREADY_LOCKED=true \
    PATH=/usr/local/bin:/usr/local/flutter/bin:/usr/local/flutter/bin/cache/dart-sdk/bin:/usr/sbin:/usr/bin:/sbin:/bin

USER 10001:10001

RUN bash -lc 'test "$(id -u)" = 10001 \
    && test "$(command -v flutter)" = /usr/local/bin/flutter \
    && test "$(command -v dart)" = /usr/local/bin/dart \
    && test "$(command -v opencode)" = /usr/local/bin/opencode \
    && command -v node \
    && command -v npm \
    && command -v trivy \
    && flutter create --project-name runner_smoke --platforms=android,ios /tmp/runner_smoke \
    && cd /tmp/runner_smoke \
    && flutter analyze \
    && flutter test \
    && cd /workspace \
    && rm -rf /tmp/runner_smoke'

USER root

RUN cp -a /home/opencode/.pub-cache /opt/flutter-pub-cache-seed \
    && touch /home/opencode/.pub-cache/.runner-seeded \
    && chown -R root:root "${FLUTTER_HOME}"

USER 10001:10001

ENTRYPOINT ["runner-entrypoint"]
CMD ["bash"]
