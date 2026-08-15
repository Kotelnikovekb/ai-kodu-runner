# Как создавать новые tool images

## Назначение

Это образы инструментов для `ai-kodu-runner`, а не образы приложений. Новый
образ нужен, когда проекту требуется отдельный runtime или набор CLI: Go,
Rust, Java, PHP и т. п.

## Что должно быть в контейнере

Обязательно:

- нужный runtime с фиксированной версией;
- `bash`, `git`, `curl`, `ca-certificates`;
- OpenCode CLI;
- все CLI, которые вызываются в `workflow.verifiers`;
- рабочий каталог `/workspace`;
- пользователь `opencode` без root-прав;
- UID/GID пользователя `10001`;
- `HOME`, `XDG_*` и кэши под `/home/opencode`;
- стабильный `/usr/local/bin/opencode` с фиксированной версией;
- общий `runner-entrypoint.sh`;
- build-time и read-only runtime smoke tests.

Пример: если verifier запускает `ruff`, `pytest`, `npm`, `trivy`, `cargo` или
`flutter`, соответствующая команда должна работать сразу после запуска образа.
Не рассчитывайте на наличие утилиты в Docker host.

## Новый Dockerfile

Создайте `images/<name>-opencode.Dockerfile`:

```dockerfile
FROM <official-runtime-image>

ARG OPENCODE_VERSION=1.18.18

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
    PATH="/home/opencode/.local/bin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git \
    && useradd --create-home --uid 10001 --user-group --shell /bin/bash opencode \
    && mkdir -p /home/opencode/.config/opencode \
        /home/opencode/.local/share/opencode \
        /home/opencode/.local/state \
        /home/opencode/.cache/opencode \
        /home/opencode/.cache/npm /workspace \
    && chown -R 10001:10001 /home/opencode /workspace \
    && rm -rf /var/lib/apt/lists/*

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN curl -fsSL https://opencode.ai/install \
        | bash -s -- --version "${OPENCODE_VERSION}" --no-modify-path \
    && install -m 0755 /home/opencode/.opencode/bin/opencode /usr/local/bin/opencode \
    && rm -rf /home/opencode/.opencode

COPY --chmod=0755 runner-entrypoint.sh /usr/local/bin/runner-entrypoint

WORKDIR /workspace
USER 10001:10001

RUN bash -lc 'test "$(id -u)" = 10001 \
    && test "$(command -v opencode)" = /usr/local/bin/opencode \
    && opencode --version'

ENTRYPOINT ["runner-entrypoint"]
CMD ["bash"]
```

Не помещайте в Dockerfile ключи, токены и пароли. Не подключайте
`/var/run/docker.sock` и не используйте privileged-режим для исправления
проблем образа. Очищайте apt-кэш в том же слое, где он создаётся.

## Подключение к GitLab CI

Добавьте в `.gitlab-ci.yml`:

```yaml
      - IMAGE_NAME: go-opencode
        DOCKERFILE: images/go-opencode.Dockerfile
```

И добавьте Dockerfile в lint job:

```yaml
    - hadolint images/go-opencode.Dockerfile
```

После этого GitLab будет собирать образ параллельно с остальными и публиковать:

```text
<namespace>/go-opencode:<commit-sha>
<namespace>/go-opencode:<branch-slug>
<namespace>/go-opencode:latest
```

## Локальная проверка

```bash
IMAGE=go-opencode
FILE=images/go-opencode.Dockerfile
docker build --pull -f "$FILE" -t "$IMAGE:local" .
docker run --rm "$IMAGE:local" sh -lc \
  'id && pwd && git --version && opencode --version'
# Сначала добавьте kind `go` и его обязательные CLI в smoke-test.sh.
./smoke-test.sh "$IMAGE:local" go
```

Затем проверьте runtime и все verifier-команды нового типа проекта.

## Использование в job

Локально:

```json
{ "image": "go-opencode:local" }
```

Из Docker Hub:

```json
{ "image": "my-user/go-opencode:2026.08.0" }
```

В production используйте digest:

```text
my-user/go-opencode@sha256:<digest>
```

Секреты передавайте только через `environment_from_runner` или `secrets`; не
добавляйте их в prompt, Dockerfile или image.

## Checklist

- [ ] Dockerfile лежит в `images/`.
- [ ] `IMAGE_NAME` совпадает с именем образа.
- [ ] Dockerfile добавлен в hadolint.
- [ ] Есть runtime, shell, git, curl, OpenCode и verifier CLI.
- [ ] Есть `/workspace`, рабочий пользователь и корректный `PATH`.
- [ ] OpenCode state содержит весь каталог `.../share/opencode`, а не только `log`.
- [ ] Writable language caches находятся в Runner tmpfs или `/workspace/.cache`.
- [ ] Используется общий exec-form `runner-entrypoint`.
- [ ] Нет секретов, Docker socket и privileged-зависимостей.
- [ ] Прошли локальная сборка и read-only smoke test.
- [ ] Новый image kind и его обязательные CLI добавлены в `smoke-test.sh`.
- [ ] Образ проверен на `amd64` и `arm64`, если нужна multi-arch публикация.
