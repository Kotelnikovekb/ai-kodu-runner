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
```

В GitLab добавьте masked/protected variables `DOCKERHUB_USERNAME` и
`DOCKERHUB_TOKEN`. Pipeline собирает образы параллельно и публикует commit SHA,
branch tag и `latest` только из default branch. Для Git tag публикуется также
релизный тег. Сборка создаёт multi-arch manifest для `linux/amd64` и
`linux/arm64`.

## GitHub Actions

Workflow [`tool-images.yml`](../.github/workflows/tool-images.yml) запускается
только если изменились `tool-images/**` или сам workflow. Для Pull Request он
только собирает все образы под `linux/amd64` без публикации. После push в
`main`/`master` каждый образ собирается отдельной matrix job и публикуется в
Docker Hub как multi-arch manifest (`linux/amd64` + `linux/arm64`).

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
