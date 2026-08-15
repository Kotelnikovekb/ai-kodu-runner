# Kodu Runner tool images

Готовые tool images для `ai-kodu-runner`:

- `flutter-opencode` — Flutter/Dart, Node.js и OpenCode;
- `nextjs-opencode` — Node.js, OpenCode и Trivy;
- `python-opencode` — Python 3.12, Node.js и OpenCode.
- `php-opencode` — PHP 8.4, Composer, Node.js и OpenCode;
- `universal-opencode` — базовые CLI-инструменты и OpenCode.

Локальная сборка из этого каталога:

```bash
docker build -f images/flutter-opencode.Dockerfile -t flutter-opencode:local .
docker build -f images/nextjs-opencode.Dockerfile -t nextjs-opencode:local .
docker build -f images/python-opencode.Dockerfile -t python-opencode:local .
docker build -f images/php-opencode.Dockerfile -t php-opencode:local .
docker build -f images/universal-opencode.Dockerfile -t universal-opencode:local .
```

## Общий runtime-контракт

Все образы:

- запускаются пользователем `opencode` с UID/GID `10001`;
- устанавливают фиксированную версию OpenCode в `/usr/local/bin/opencode`;
- сохраняют `HOME` и XDG-каталоги под `/home/opencode`;
- используют `OPENCODE_DB=:memory:` и отключают auto-update;
- содержат полный `/home/opencode/.local/share/opencode`, включая snapshots;
- запускаются через `runner-entrypoint`, который подготавливает writable runtime
  directories и, для Flutter, восстанавливает подготовленный pub cache;
- проходят build-time проверку login-shell PATH и обязательных инструментов;
- рассчитаны на read-only root filesystem с tmpfs mounts Runner.

Language-specific dependency caches, которым не соответствует отдельный tmpfs
Runner, направлены в writable `/workspace/.cache`.

После локальной сборки проверьте тот же read-only контракт, который использует
Runner:

```bash
./smoke-test.sh flutter-opencode:local flutter
./smoke-test.sh nextjs-opencode:local nextjs
./smoke-test.sh python-opencode:local python
./smoke-test.sh php-opencode:local php
./smoke-test.sh universal-opencode:local universal
```

Smoke test запускает образ без сети и с read-only rootfs, проверяет UID, PATH и
запись в каждый OpenCode tmpfs path. Это защищает от повторения ошибки
`EROFS` при создании `/home/opencode/.local/share/opencode/snapshot`.

В GitLab добавьте masked/protected variables `DOCKERHUB_USERNAME` и
`DOCKERHUB_TOKEN`. Pipeline собирает образы параллельно и публикует commit SHA,
branch tag и `latest` только из default branch. Для Git tag публикуется также
релизный тег. Сборка создаёт multi-arch manifest для `linux/amd64` и
`linux/arm64`.

## GitHub Actions

Workflow [`tool-images.yml`](../.github/workflows/tool-images.yml) запускается
только если изменились `tool-images/**` или сам workflow. Для Pull Request он
собирает все образы под `linux/amd64` без публикации и выполняет read-only
smoke test. После push в
`main`/`master` каждый образ собирается отдельной matrix job и публикуется в
Docker Hub как multi-arch manifest (`linux/amd64` + `linux/arm64`). Затем
опубликованный `linux/amd64` образ проходит тот же smoke test.

В настройках GitHub Repository добавьте:

- variable `DOCKERHUB_NAMESPACE` — username или organization в Docker Hub
  (необязательно, по умолчанию `kotelnikoffdev`);
- secret `DOCKERHUB_USERNAME`;
- secret `DOCKERHUB_TOKEN` с правом записи.

После публикации workflow скачивает каждый образ на обычный GitHub-hosted
runner и сканирует его через Trivy. Docker socket внутрь job-контейнера не
передаётся. Образ сканируется по полному адресу вида
`kotelnikoffdev/flutter-opencode:<commit-sha>`, поэтому имя не превращается в
`/flutter-opencode:latest` при отсутствии repository variable.

Основной workflow runner-проекта имеет `paths-ignore: tool-images/**`, поэтому
изменение только Dockerfile или документации образов не запускает Rust CI.
Изменение только runner-файлов не запускает `tool-images.yml`.

Подробные правила создания новых образов находятся в
[`CONTRIBUTING.md`](CONTRIBUTING.md).

Лицензия проекта — Apache-2.0. Сторонние компоненты перечислены в
[`../THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
