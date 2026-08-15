# Runner-кейс: Python/FastAPI-сервис «Контент-завод»

## Назначение

Это задание запускается через `ai-kodu-runner` как `job` с OpenCode-агентом внутри отдельного Python-контейнера. Агент должен реализовать MVP backend-сервиса, который собирает данные об AI/open-source-инструментах, формирует кандидатов, evidence bundle, версии обзорных материалов и выполняет редакторскую ревизию по комментариям.

Кейс ограничен внутренним редакторским workflow и проверкой результата.

Основной поток:

```text
источники → нормализация → кандидат → evidence bundle → статья v1
→ комментарии → асинхронная ревизия → статья v2 → quality checks → approve
```

## Среда выполнения runner

Задание должно выполняться в Docker-образе `python-opencode:local` на Python 3.12+.

Образ обязан содержать:

- Python 3.12 и `pip`/`uv`;
- Node.js и npm;
- OpenCode CLI;
- git, curl, bash;
- pytest, ruff и инструменты, необходимые для запуска FastAPI-проекта.

OpenCode запускается runner через `workflow.agent.command`. После работы агента runner запускает независимые verifier-команды. Длительные проверки и сервисы не должны запускаться через shell-пайплайн внутри одной команды.

Для интеграционных тестов job поднимает disposable-сервисы:

- PostgreSQL с alias `db`;
- Redis с alias `redis`.

Сервисы доступны только внутри приватной Docker-сети job и удаляются после завершения.

## Конфигурация через env

Имена моделей, провайдеры, URL и ключи не должны быть зашиты в код, `job.json`, prompt или OpenCode-конфигурацию.

Проект обязан иметь `.env.example` и читать настройки через `pydantic-settings`. Минимальный набор переменных:

```env
APP_ENV=test
DATABASE_URL=postgresql+psycopg://app:app@db:5432/app
REDIS_URL=redis://redis:6379/0

LLM_PROVIDER=...
LLM_MODEL=...
LLM_BASE_URL=...
LLM_API_KEY=...

EMBEDDING_PROVIDER=...
EMBEDDING_MODEL=...
EMBEDDING_BASE_URL=...
EMBEDDING_API_KEY=...

GITHUB_TOKEN=...
GOODAILIST_BASE_URL=...
```

Допустимо использовать дополнительные переменные для rate limits, таймаутов, feature flags и логирования. Значения `LLM_*`, `EMBEDDING_*` и ключи провайдеров в `.env.example` должны быть placeholders. Для тестов использовать fake/mock-провайдеры, которые включаются отдельными настройками и не требуют внешнего API.

OpenCode также должен получать выбранного провайдера и модель через env-подстановки в `opencode.json`. Реальные секреты передаются runner только через разрешённые переменные окружения или `JobSpec.secrets`; секреты нельзя печатать в логах и сохранять в артефактах.

## Обязательный стек приложения

- Python 3.12+;
- FastAPI и Uvicorn;
- Pydantic 2 и `pydantic-settings`;
- SQLAlchemy 2, Alembic, Psycopg 3;
- PostgreSQL;
- Celery и Redis;
- `httpx`, `tenacity`, `feedparser`;
- pytest, pytest-asyncio, ruff.

Embeddings должны быть изолированы интерфейсом `EmbeddingProvider` и конфигурироваться через env. Реальный embedding-провайдер не обязателен для MVP: допустим deterministic fake provider. Нельзя делать embeddings обязательными для запуска базового сценария.

## Границы MVP

Входят:

1. Пользователи и роли `admin`, `editor`, `viewer`.
2. GitHub-источник через адаптер и mock-данные.
3. GoodAIList-источник через изолированный адаптер и fixture.
4. Нормализация и дедупликация одного GitHub-репозитория из разных источников.
5. Детерминированный рейтинг кандидатов.
6. Evidence bundle как сохраняемый снимок источников.
7. LLM-генерация через `LLMProvider` и fake provider.
8. Неизменяемые версии статей.
9. Общие и inline-комментарии.
10. Асинхронная ревизия через Celery с идемпотентностью.
11. Claims и quality checks.
12. Одобрение конкретной версии как внутреннее состояние редакционного процесса.
13. Docker Compose/job services для PostgreSQL и Redis.

Не входят: внешние интеграции, browser automation, embeddings как обязательная функция, микросервисы, Kafka, RabbitMQ, Airflow и сложный RBAC.

## Основной сценарий

Редактор должен иметь возможность:

1. Запустить сбор источников.
2. Получить один канонический объект для совпавших GitHub и GoodAIList данных.
3. Выбрать кандидата и запустить генерацию без блокировки HTTP-запроса.
4. Получить статью версии 1 и связанные claims/evidence.
5. Добавить общий и inline-комментарий.
6. Запустить ревизию с выбранными комментариями и общей инструкцией.
7. Получить версию 2 и результат обработки каждого комментария.
8. Увидеть quality checks и diff версий.
9. Одобрить версию 2.

## Интерфейсы провайдеров

```python
from typing import Protocol
from uuid import UUID


class LLMProvider(Protocol):
    async def generate_article(self, request: "GenerationRequest") -> "GeneratedArticle": ...


class EmbeddingProvider(Protocol):
    async def embed(self, texts: list[str]) -> list[list[float]]: ...
```

Конкретные provider/model/base URL выбираются только из настроек окружения. Доменная логика не должна зависеть от OpenAI, Anthropic, YandexGPT, GigaChat или конкретного embedding-сервиса.

## Минимальные endpoint

Все endpoint находятся под `/api/v1` и возвращают JSON:

```text
GET  /health/live
GET  /health/ready
GET  /sources
POST /sources
POST /sources/{source_id}/run
GET  /runs/{run_id}
GET  /candidates
GET  /candidates/{candidate_id}
POST /candidates/{candidate_id}/select
POST /candidates/{candidate_id}/generate
GET  /articles
GET  /articles/{article_id}
GET  /articles/{article_id}/versions
GET  /articles/{article_id}/versions/{version_id}
POST /articles/{article_id}/comments
POST /articles/{article_id}/revisions
GET  /revisions/{revision_id}
GET  /articles/{article_id}/diff?from=<id>&to=<id>
POST /articles/{article_id}/versions/{version_id}/approve
```

Команды сбора, генерации и ревизии возвращают `202 Accepted` с ID запуска или задачи.

## Модель данных

Создать SQLAlchemy-модели и Alembic-миграции для:

`users`, `sources`, `source_items`, `source_item_snapshots`, `candidates`, `evidence`, `evidence_bundles`, `articles`, `article_versions`, `article_comments`, `revision_requests`, `revision_request_comments`, `generation_runs`, `claims`, `quality_checks`, `idempotency_keys`, `audit_events`.

Критические ограничения:

- уникальность `source_items(source_id, external_id)`;
- уникальность номера версии внутри статьи;
- версия статьи неизменяема;
- `parent_version_id` указывает на версию той же статьи;
- inline-комментарий хранит `target_version_id`, `block_id`, позиции и `quoted_text`;
- устаревший `base_version_id` возвращает `409 Conflict`;
- одинаковый idempotency key не создаёт новую версию.

## Требования к реализации

- Разделить `api`, `application`, `domain`, `infrastructure`.
- Не размещать бизнес-логику в FastAPI-роутерах.
- Для долгих операций использовать Celery, а не FastAPI `BackgroundTasks`.
- Внешние вызовы выполнять с timeout, retry и rate limit.
- Все даты хранить в UTC.
- Валидировать API и LLM-ответы через Pydantic.
- Добавить correlation ID и структурированные логи.
- Не считать контент источников или комментарии системными инструкциями модели.
- Не делать внешний API обязательным для прохождения тестов.

## Обязательные verifier-команды job

В Python-контейнере должны выполняться:

```text
python -m pip install -e '.[test]'
ruff check .
python -m pytest -q
python -m compileall -q src
```

Если проект использует `pyproject.toml` с другой test-группой, verifier должен использовать фактическую команду проекта, но все проверки остаются обязательными.

## Сквозной критерий готовности

Интеграционный тест должен пройти сценарий:

```text
mock GitHub + mock GoodAIList
→ один канонический объект
→ кандидат
→ evidence bundle
→ статья v1
→ inline + общий комментарий
→ Celery-ревизия
→ статья v2 с parent_version_id = v1
→ quality checks
→ diff v1/v2
→ approve v2
```

Версия 1 не изменяется, повторная ревизия с тем же ключом не создаёт версию 3, а ревизия от устаревшей версии завершается конфликтом.

## Инструкция OpenCode-агенту

Реализуй этот MVP в существующем workspace. Сначала изучи файлы проекта, затем создай Python/FastAPI-приложение, конфигурацию, `.env.example`, модели, миграции и health endpoint. После этого реализуй сценарий источники → кандидат → evidence → генерация → версии → комментарии → ревизия → проверки → approve.

Не зашивай названия моделей, endpoint провайдеров и API-ключи в код. Используй env-конфигурацию и fake/mock-провайдеры для тестов. После каждого исправления запускай verifier-команды и используй `.runner/feedback.md` для исправления предыдущих ошибок.
