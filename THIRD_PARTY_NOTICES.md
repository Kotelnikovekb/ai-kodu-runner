# Third-party notices

Copyright 2026 Kotelnikovekb

Этот файл перечисляет основные сторонние проекты, которые используются в
исходном коде или собираемых `tool-images`. Полные тексты лицензий сторонних
компонентов поставляются самими компонентами и должны сохраняться внутри
соответствующих base images.

## Runtime и tool images

- Rust и Cargo ecosystem — лицензии указываются соответствующими пакетами в `Cargo.lock`.
- Flutter — [Flutter](https://github.com/flutter/flutter), BSD-3-Clause.
- Dart — поставляется вместе с Flutter SDK, BSD-3-Clause.
- Android SDK и command-line tools — лицензии Google для соответствующих компонентов Android SDK.
- Node.js — [Node.js](https://github.com/nodejs/node), MIT.
- PHP — [PHP](https://www.php.net/), PHP License 3.01.
- Python — [Python](https://github.com/python/cpython), PSF License.
- Debian — [Debian](https://www.debian.org/), лицензии отдельных пакетов.
- OpenCode — [OpenCode](https://github.com/anomalyco/opencode), лицензия определяется версией upstream-проекта.
- Trivy — [Trivy](https://github.com/aquasecurity/trivy), Apache-2.0.
- Composer — [Composer](https://github.com/composer/composer), MIT.

Образы также включают транзитивные пакеты, установленные через `apt`, `npm`,
Composer, `pip` и Cargo. Их точный состав зависит от версии base image и
lock-файлов. Перед релизом образа необходимо сгенерировать SBOM и проверить
лицензии фактически опубликованного image.

При добавлении новой зависимости обновите этот файл, сохраните исходную
лицензию в распространяемом артефакте и проверьте ограничения на публикацию
Docker image. Не переносите лицензию стороннего проекта на код
`ai-kodu-runner`.

