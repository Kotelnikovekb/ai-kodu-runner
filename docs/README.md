# Документация AI Kodu Runner

Корневой [`README.md`](../README.md) — краткий вход в проект и quick start.
Здесь собраны долговечные проектные документы; исторические проверки находятся
в [`audits/`](audits/), архитектурные решения — в [`decisions/`](decisions/).

## Архитектура и планы

- [`ROADMAP.md`](ROADMAP.md) — целевая архитектура и этапы реализации.
- [`GLOSSARY.md`](GLOSSARY.md) — единая терминология проекта.
- [`SECURITY_MODEL.md`](SECURITY_MODEL.md) — Enterprise security model и trust boundaries.
- [`SUPPORT.md`](SUPPORT.md) — qualification baseline и поддерживаемые конфигурации.
- [`VERSIONING.md`](VERSIONING.md) — совместимость Rust API и протоколов.
- [`CRATES_IO_RELEASE.md`](CRATES_IO_RELEASE.md) — checklist публикации public crates.
- [`COMMUNITY_OPERATIONS.md`](COMMUNITY_OPERATIONS.md) — локальный и
  self-hosted CI режим Community Edition.
- [`openapi/community-control-plane.yaml`](openapi/community-control-plane.yaml)
  — public HTTP control-plane contract.
- [`schemas/`](schemas/) — versioned JSON schemas для JobSpec, JobResult, logs и
  стабильного `FailureInfo`.

## Процессы и материалы поставки

- [`COMMERCIAL_DISTRIBUTION.md`](COMMERCIAL_DISTRIBUTION.md) — коммерческая дистрибуция и attribution.
- [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) — сторонние компоненты и notices.
- [`IMPLEMENTATION_CASE.md`](IMPLEMENTATION_CASE.md) — пример сквозного runner-кейса.

## Аудиты

- [`audits/README.md`](audits/README.md) — текущий статус и порядок чтения.

## Архитектурные решения

- [`decisions/`](decisions/) — принятые ADR и их обоснования.

## Файлы, которые намеренно остаются в корне

`AGENTS.md`, `LICENSE.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` и `SECURITY.md`
сохраняются в корне из-за области действия инструкций, стандартов хостинга и
процесса disclosure.
