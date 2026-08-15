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
- `HOME`, `XDG_*` и кэши под `/home/opencode`.

Пример: если verifier запускает `ruff`, `pytest`, `npm`, `trivy`, `cargo` или
`flutter`, соответствующая команда должна работать сразу после запуска образа.
Не рассчитывайте на наличие утилиты в Docker host.

## Новый Dockerfile

Создайте `images/<name>-opencode.Dockerfile`:

```dockerfile
FROM <official-runtime-image>

ENV HOME=/home/opencode \
    XDG_CONFIG_HOME=/home/opencode/.config \
    XDG_DATA_HOME=/home/opencode/.local/share \
    XDG_CACHE_HOME=/home/opencode/.cache \
    XDG_STATE_HOME=/home/opencode/.local/state \
    OPENCODE_DB=:memory: \
    PATH="/home/opencode/.local/bin:/home/opencode/.opencode/bin:${PATH}"

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash opencode \
    && mkdir -p /home/opencode/.config /home/opencode/.local/share \
        /home/opencode/.local/state /home/opencode/.cache /workspace \
    && chown -R opencode:opencode /home/opencode /workspace

USER opencode
RUN curl -fsSL https://opencode.ai/install | bash

WORKDIR /workspace
ENTRYPOINT []
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
- [ ] Нет секретов, Docker socket и privileged-зависимостей.
- [ ] Прошли локальная сборка и smoke test.
- [ ] Образ проверен на `amd64` и `arm64`, если нужна multi-arch публикация.

