# Roadmap AI Kodu Runner

## 1. Назначение документа

Этот документ фиксирует продуктовый и технический план развития AI Kodu Runner
по модели open core:

- Community Edition — локальный и self-hosted Docker executor под Apache-2.0;
  - Enterprise Edition — коммерческий Kubernetes executor для изолированного
    выполнения недоверенных заданий с Kata Containers;
  - обе редакции используют общий versioned Job Protocol и одинаковую модель
    workflow, результатов, логов и артефактов;
  - безопасность выбирается явно через требования задания и никогда не понижается
    через неявный fallback.

Roadmap является рабочим документом. Оценки сроков уточняются после технических
исследований и первых end-to-end прототипов.

## 2. Продуктовая модель

### Community Edition

Публичный репозиторий, лицензия Apache-2.0:

- CLI и локальный daemon;
  - Docker Engine и Docker Desktop;
  - JobSpec, workflow и обязательные verifiers;
  - локальные, архивные и Git workspaces;
  - публикация изменений в Git;
  - потоковые логи, диагностика и артефакты;
  - локальная политика ресурсов, сети и переменных окружения;
  - готовые tool images и примеры запуска.

Community Edition предназначена для локальной разработки, CI и выполнения
доверенных или контролируемых заданий. Docker-контейнер не считается достаточной
tenant-изоляцией для заведомо вредоносного кода.

### Enterprise Edition

Закрытый коммерческий репозиторий и отдельная коммерческая лицензия:

- Kubernetes executor/controller;
  - Kubernetes Jobs;
  - обязательный Kata RuntimeClass;
  - выделенный sandbox node pool;
  - Helm chart и preflight-проверки кластера;
  - RBAC, Pod Security Admission, NetworkPolicy и quotas;
  - tenant isolation и admission policies;
  - credential proxy и ссылки на секреты вместо передачи значений;
  - audit trail, observability и автоматическая очистка;
  - autoscaling, upgrade/rollback lifecycle и enterprise support;
  - проверенная матрица Kubernetes, containerd и Kata Containers.

Коммерческая ценность продукта — не само поле `runtimeClassName`, а готовая,
проверенная и поддерживаемая система безопасной эксплуатации.

## 3. Архитектурные принципы

1. Общий протокол не зависит от Docker или Kubernetes.
   2. Executor выбирается control plane и локальной политикой, а не произвольным
      клиентским значением.
   3. Job описывает необходимые capabilities и уровень изоляции.
   4. Если job требует `sandboxed`, несовместимый executor отклоняет его до запуска.
   5. Fallback с Kata на обычный `runc` запрещён.
   6. Все операции завершения, публикации и загрузки артефактов идемпотентны.
   7. Пользовательский JobSpec не может передавать произвольный Docker HostConfig,
      Kubernetes PodSpec, ServiceAccount, volume, host namespace или устройство.
   8. Все workload images в production закрепляются по digest.
   9. Секреты по возможности не попадают внутрь sandbox в исходном виде.
   10. Политики безопасности применяются fail closed.
   11. PostgreSQL primary является каноническим реестром Enterprise attempts и
       линеаризуемым источником start permit; controller остаётся stateless или
       восстанавливаемым.
   12. Threat model и Definition of Secure Done применяются до реализации
       Enterprise trust boundaries, а не только перед релизом.

## 4. Целевая структура репозиториев

### Public repository

```text
ai-kodu-runner/
├── crates/
│   ├── runner-protocol/
│   ├── runner-core/
│   ├── runner-cli/
│   └── executor-docker/
├── images/
├── examples/
├── docs/
│   └── decisions/
└── tests/
```

Назначение crates:

- `runner-protocol` — JobSpec, JobResult, события, capabilities, версионирование
  и JSON compatibility fixtures;
  - `runner-core` — workflow engine, policy interfaces, workspace/artifact
    contracts, lifecycle и общие utilities;
  - `executor-docker` — только Docker-specific реализация;
  - `runner-cli` — сборка Community Edition и выбор executor через factory.

### Private enterprise repository

```text
ai-kodu-runner-enterprise/
├── crates/
│   ├── enterprise-runner/
│   ├── executor-kubernetes/
│   ├── runner-worker/
│   ├── event-ingestion/
│   ├── attempt-registry/
│   ├── workspace-preparer/
│   ├── git-publisher/
│   ├── model-gateway/
│   └── credential-proxy/
├── charts/
│   └── ai-kodu-runner-enterprise/
├── policies/
│   ├── admission/
│   ├── network/
│   └── pod-security/
├── tests/
│   ├── conformance/
│   ├── e2e/
│   └── security/
└── docs/
```

Enterprise repository зависит от опубликованной и явно закреплённой версии
public crates. Обновление public core сопровождается compatibility и
integration tests в Enterprise CI.

## 5. Целевой контракт задания

Поле `executor: "docker"` необходимо постепенно заменить декларативными
требованиями:

```json
{
  "api_version": "ai-kodu-runner.dev/v1beta1",
  "execution": {
    "isolation": "sandboxed",
    "capabilities": ["flutter", "android-build"]
  }
}
```

Предварительные уровни изоляции:

- `container` — обычный контейнер для доверенных workloads;
  - `sandboxed` — отдельный sandbox runtime, обязательный для недоверенного кода;
  - `dedicated` — выделенный tenant/node pool, если это разрешено тарифом и
    конфигурацией установки.

Предварительные capabilities:

- `network`;
  - `services`;
  - `streaming_logs`;
  - `cancellation`;
  - `git_publish`;
  - `sandboxed`;
  - `kata`;
  - `untrusted_workload`;
  - `multi_tenant`;
  - toolchain capabilities: `flutter`, `android-build`, `node`, `python` и другие.

Миграция с `v1alpha1` должна сохранять чтение старых Community job manifests в
течение заранее объявленного периода совместимости.

`ExecutionRequirements` также задаёт ограниченные ресурсы, включая CPU, memory,
PID, timeout, `ephemeral_storage_mb` и `workspace_mb`. Kubernetes backend
применяет одновременно container request/limit для `ephemeral-storage` и
`emptyDir.sizeLimit`; memory-backed `emptyDir` учитывается в memory budget.

## 6. Целевой контракт Executor

Ориентировочный интерфейс:

```rust
#[async_trait]
pub trait Executor: Send + Sync {
    fn capabilities(&self) -> ExecutorCapabilities;
    async fn doctor(&self) -> Result<DoctorReport>;
    async fn run(
        &self,
        job: JobSpec,
        events: EventSink,
        cancel: CancellationToken,
    ) -> Result<JobResult>;
    async fn cleanup(&self, job: JobIdentity) -> Result<()>;
}
```

Дополнительно нужен `ExecutorFactory`, чтобы CLI и daemon не импортировали
`DockerExecutor` напрямую. Conformance suite должна проверять одинаковую
семантику lifecycle, timeout, cancellation, логов, результатов и cleanup для
каждой реализации.

## 7. Целевая модель Enterprise delivery

ADR 0003 фиксирует основной канал доставки и границы доверия:

```text
runner-worker
    ├── stdout/stderr → Kubernetes logs (диагностический fallback)
    ├── events → mTLS ingestion → durable event store
    ├── bounded WAL → emptyDir (только временный retry buffer)
    └── result/artifacts → object storage
```

Один product attempt соответствует одному Kubernetes Job. Каждый созданный Pod
имеет отдельный `execution_id` и до запуска пользовательского кода получает
compare-and-set start permit. Повторный Pod не исполняет тот же attempt; retry
создаётся новым Job и новым attempt.

`completed` возможен только после состояния `result_durable`, когда result
manifest, обязательные события и объекты проверены по digest. Git publication —
отдельная идемпотентная операция trusted publisher-а.

## 8. План реализации

### Этап 0. Зафиксировать решения и границы

Цель: исключить архитектурные и лицензионные разночтения до появления закрытого
кода.

- [x] Добавить ADR `0002-community-docker-enterprise-kubernetes.md`.
  - [x] Добавить ADR `0003-enterprise-delivery-and-trust-boundaries.md`.
  - [x] Добавить ADR `0004-enterprise-threat-model-and-secure-delivery-gates.md`.
  - [x] Создать ранний `SECURITY_MODEL.md` с assets, actors, trust zones,
    residual risks и Definition of Secure Done.
  - [x] Зафиксировать состав Community и Enterprise редакций.
  - [x] Зафиксировать модель двух репозиториев и правила зависимостей.
  - [x] Выбрать схему версионирования public protocol и crates.
  - [x] Зафиксировать отсутствие fallback с Kata на `runc`.
  - [x] Определить, кто выбирает executor и проверяет capabilities.
  - [x] Определить минимальную поддерживаемую Kubernetes-матрицу.
  - [x] Подготовить process для commercial EULA, attribution и NOTICE; финальная
    EULA требует отдельного одобрения юриста перед внешним релизом.
  - [x] Определить contributor policy: DCO, без CLA до отдельного решения.

Критерий готовности: решения приняты и описаны, граница открытого и закрытого
кода однозначна. Финальная EULA остаётся внешним юридическим release gate и не
считается подготовленной до одобрения counsel.

Threat model проходит initial review до создания Enterprise skeleton и затем
обновляется при изменении trust boundary, credential/egress scope, tenant
sharing, protocol authority или sandbox runtime/filesystem.

### Этап 1. Подготовить Community core

Цель: сделать Kubernetes backend подключаемым без изменений Docker-specific
кода и без `if enterprise` в общем runtime.

- [x] Преобразовать корневой crate в Cargo workspace.
  - [x] Вынести `runner-protocol`.
  - [x] Вынести `runner-core`.
  - [x] Вынести `executor-docker`.
  - [ ] Перенести бинарный entrypoint в `runner-cli` (отложено: не блокирует M2).
  - [x] Добавить `ExecutorFactory`.
  - [x] Убрать прямое создание `DockerExecutor` из CLI и daemon.
  - [x] Разделить общую и Docker-specific конфигурацию.
  - [x] Перенести Docker cleanup в реализацию executor.
  - [x] Добавить capabilities и `DoctorReport`.
  - [x] Добавить versioned JSON fixtures для JobSpec и JobResult.
  - [x] Сохранить поведение и CLI Community Edition без регрессий.

Критерии готовности:

- [x] Community binary собирается только с Docker backend;
  - [x] daemon работает через `Arc<dyn Executor>`;
  - [x] public core не зависит от `bollard`;
  - [x] текущие Docker tests проходят без изменения пользовательского поведения;
  - [x] новый mock executor запускается через ту же factory/conformance suite.

**Статус:** ✅ ЗАВЕРШЕН (2026-08-16). См. `AUDIT_ROADMAP_UPDATED.md` для деталей.

### Этап 2. Версионировать protocol и control-plane API

Цель: получить стабильный контракт между control plane, runner и job worker.

- [x] Ввести typed `ExecutionRequirements` вместо строкового выбора executor.
  - [ ] Ввести typed network policy вместо произвольной строки.
  - [x] Добавить capability registration runner-а.
  - [x] Добавить heartbeat и продление lease.
- [x] Добавить серверную отмену job (typed heartbeat response с bounded
  heartbeat-failure policy).
  - [x] Сделать completion идемпотентным (Idempotency-Key).
- [ ] Добавить `ephemeral_storage_mb` и `workspace_mb` в resource contract.
- [x] Ограничить Community workspace/archive ingestion по размеру и количеству
  файлов; применять лимиты fail-closed до запуска контейнера.
- [x] Протянуть job deadline/cancellation через workspace preparation и Git
  publication.
- [x] Ограничить и обезличить failure diagnostics; не экспортировать prompt,
  `AGENTS.md` и feedback files.
- [x] Ограничить daemon HTTP calls/completion retries и корректно отражать
  потерю streaming logs в `JobResult.log_truncated`.
- [x] Зафиксировать Community control-plane API в OpenAPI 3.1 и versioned JSON
  schemas для `v1alpha1`/`v1beta1` JobSpec, log chunk и JobResult.
- [x] Добавить локальный SQLite completion outbox с идемпотентной записью,
  bounded retry и replay после рестарта daemon; shared Enterprise ingestion
  queue остаётся отдельным control-plane компонентом.
- [x] Сделать runner identity стабильной на весь процесс и cleanup truthful для
  active/неудалённых Docker resources.
  - [x] Добавить typed failures: `FailureInfo { kind, code, message }`.
  - [ ] Определить event envelope: `job_id`, `attempt`, `run_id`, `execution_id`,
    stream, sequence, `event_id` и protocol version.
  - [ ] Добавить at-least-once delivery, durable ACK, deduplication и bounded
    out-of-order window.
  - [ ] Определить negotiation для event/WAL/gap windows, `resync_required`,
    replay от highest contiguous ACK и fail-closed WAL exhaustion.
  - [ ] Определить projected Pod-bound bootstrap token с audience
    `ai-kodu-ingestion`, `TokenReview`, direct Pod validation и обменом на scoped
    execution token только после start permit.
  - [ ] Определить scoped execution token для event append, фиксированных object
    keys и одного terminal result.
  - [ ] Зафиксировать `execution_id` как полный Kubernetes Pod UID без
    хеширования или усечения.
  - [ ] Разделить user-code deadline, finalization grace, termination margin и
    внешний Kubernetes deadline в protocol/resource contract.
  - [ ] Ввести состояния `finalizing`, `result_durable` и `publishing`.
  - [x] Добавить `secret_ref`; объявить inline secrets deprecated для daemon.
  - [x] Разделить user failure, policy rejection и infrastructure failure.
  - [x] Добавить стабильный `FailureInfo` schema и Docker infrastructure
    fallback `executor_operation_failed`; детальная классификация отдельных
    Docker API phases расширяется аддитивно без изменения user failure.
  - [x] Описать правила обратной совместимости `v1alpha1` и `v1beta1`.

Критерий готовности: рестарт runner-а или временный сетевой сбой не приводит к
потере результата, повторной публикации или бесконечному удержанию lease.

**Статус:** 🟡 В ПРОЦЕССЕ. Community protocol, OpenAPI contract, local completion
recovery и Docker delivery safeguards реализованы; Enterprise event envelope,
durable ACK/ingestion и Kubernetes resource contract остаются в работе.

### Этап 3. Создать Enterprise skeleton

Цель: настроить независимую разработку и поставку закрытых компонентов.

- [ ] Создать private repository.
  - [ ] Провести initial review `SECURITY_MODEL.md`; Enterprise trust-boundary
    implementation не начинается при незакрытых critical design findings.
  - [ ] Добавить commercial license headers и dependency policy.
  - [ ] Подключить закреплённые версии public crates.
  - [ ] Настроить CI для Rust, container images и Helm.
  - [ ] Добавить SBOM, vulnerability scan и image signing.
  - [ ] Добавить release manifest со всеми совместимыми версиями.
  - [ ] Настроить private OCI registry для images и Helm charts.
  - [ ] Создать stateless event-ingestion skeleton с durable store/queue adapter.
  - [ ] Создать PostgreSQL attempt-registry adapter: primary-only CAS для start
    permit и fenced publication lease; process memory/cache/read replica не могут
    быть authority.
  - [ ] До Kubernetes MVP реализовать PoC гонки двух ingestion replicas за один
    start permit.
  - [ ] Определить object-storage layout для workspace, result и artifacts.
  - [ ] Определить per-attempt/run staging prefix, immutable manifest-as-commit,
    lifecycle GC для uncommitted uploads и reconciliation потерянного terminal
    ACK по валидному manifest.
  - [ ] Реализовать пустой `KubernetesExecutor`, проходящий compile/conformance
    skeleton tests.

Критерий готовности: threat model прошёл initial review, start-permit PoC
доказал single-winner CAS, enterprise binary и chart воспроизводимо собираются
в закрытом CI, а лицензии public dependencies попадают в distribution.

### Этап 4. Kubernetes Job MVP

Цель: выполнить простой job в Kubernetes без требования Kata.

- [ ] Создавать `batch/v1 Job` с индексируемыми product labels и annotations;
  Kubernetes labels не экспортируются автоматически в Prometheus.
  - [ ] Зафиксировать identity: один attempt — один Job, Pod UID — отдельный
    `execution_id`, повторная попытка — новый Job и attempt.
  - [ ] Получать PostgreSQL-primary compare-and-set start permit до workspace
    download, proxy access и user code.
  - [ ] Проверять bootstrap token через `TokenReview`, затем сверять полный Pod UID,
    namespace, owner Job UID, labels и identities прямым чтением Kubernetes API.
  - [ ] Передавать bootstrap credential только explicit projected
    ServiceAccountToken volume с custom audience; не использовать env или
    долгоживущий Secret.
  - [ ] Устанавливать `restartPolicy: Never`.
  - [ ] Для однократного выполнения использовать `backoffLimit: 0`.
  - [ ] Добавить `podFailurePolicy` для application exits и disruption conditions;
    протестировать `DisruptionTarget → replacement Pod → permit denied → fast
    exit → FailJob` для поддерживаемой матрицы.
  - [ ] Устанавливать `activeDeadlineSeconds` как внешний safety ceiling:
    preparation + user-code deadline + finalization grace + termination margin.
  - [ ] Устанавливать `ttlSecondsAfterFinished` только после durable reconciliation
    либо перехода к ограниченной failure-retention policy.
  - [ ] Преобразовывать CPU/memory/ephemeral-storage в requests и limits и задавать
    `emptyDir.sizeLimit` для workspace/WAL.
  - [ ] Следить за Job и Pod через watch с восстановлением после reconnect.
  - [ ] Использовать Kubernetes stdout/stderr как диагностический fallback, а не
    как основной durable event channel.
  - [ ] Преобразовывать Pod/Job conditions в typed failure reasons.
  - [ ] Отменять job через контролируемое удаление Kubernetes Job.
  - [ ] Собирать результат и артефакты до TTL cleanup.
  - [ ] Восстанавливаться после рестарта enterprise controller.
  - [ ] Выполнять startup и periodic orphan reconciliation по managed labels.
  - [ ] Обеспечить идемпотентность create/watch/complete/delete.

Ограничение первого MVP: `workflow.services` может быть временно запрещён в
Kubernetes backend, пока не определён проверенный lifecycle sidecars.

Критерий готовности: success, command failure, timeout, cancellation, eviction,
controller restart и API reconnect проходят end-to-end tests.

### Этап 5. Runner worker внутри workload

Цель: не строить workflow на хрупкой последовательности Kubernetes exec-вызовов.

- [ ] Выделить общий `runner-worker` для setup/agent/verifiers; Git publication
  остаётся обязанностью отдельного trusted publisher.
  - [ ] Определить worker protocol, handshake и формат result bundle; считать
    сообщения authenticated-but-untrusted и валидировать их семантику сервером.
  - [ ] Выбрать доставку worker-а: enterprise base image или init container с
    `emptyDir`.
  - [ ] Реализовать bounded WAL, mTLS ingestion, sequence/ACK и retry с
    backpressure без silent drop canonical events.
  - [ ] Использовать обычный disk-backed `emptyDir` для WAL, не
    `emptyDir.medium: Memory`; bounded retry использует jitter и circuit breaker.
  - [ ] Реализовать replay/resync contract; отсутствие нужной WAL-записи или
    переполнение WAL завершают attempt как `infrastructure_delivery_failed`.
  - [ ] Загружать result manifest и artifacts в object storage до terminal ACK.
  - [ ] Загружать blobs первыми, immutable manifest последним; восстанавливать
    `result_durable` после потери ACK только по manifest + accepted events +
    registry/Kubernetes evidence.
  - [ ] Подготовить workspace внутри Pod без hostPath.
  - [ ] Реализовать отдельный trusted workspace-preparer вне controller и sandbox.
  - [ ] Fetch-ить точные `head_sha` и `base_sha`; не использовать безусловный
    `--depth 1`, если требуется diff-based verification.
  - [ ] Создавать immutable workspace manifest/archive с digest, size и file count.
  - [ ] Реализовать безопасную загрузку и распаковку архивов с защитой от path
    traversal, symlink/hardlink и decompression attacks.
  - [ ] Реализовать отдельный trusted Git publisher с durable fenced lease на
    normalized `(repo, ref)` и compare-and-set remote ref как второй защитой.
  - [ ] Сохранить одинаковую семантику mandatory verifiers.
  - [ ] Сохранить ограничение размера и числа артефактов.
  - [ ] Поддержать корректное завершение worker-а по SIGTERM.

Критерий готовности: один и тот же protocol fixture даёт эквивалентный
JobResult в Docker и Kubernetes с учётом документированных различий backend-ов.

### Этап 6. Kata и fail-closed sandboxing

Цель: гарантировать hardware-virtualized runtime для заданий, требующих
`sandboxed`.

- [ ] Определить поддерживаемые Kata handlers.
  - [ ] Создать RuntimeClass с корректным handler.
  - [ ] Добавить scheduling node selector и tolerations для sandbox nodes.
  - [ ] Измерить и задать RuntimeClass pod overhead.
  - [ ] Добавить обязательный `runtimeClassName` в workload template.
  - [ ] Добавить startup preflight RuntimeClass и node readiness.
  - [ ] Добавить реальный canary Pod, подтверждающий запуск через ожидаемый runtime.
  - [ ] Запрещать leasing sandboxed jobs при неуспешном preflight.
  - [ ] Запрещать fallback на default runtime при любых ошибках.
  - [ ] Добавить audit event с RuntimeClass, node и runtime evidence.
  - [ ] Измерить image pull, scheduling, sandbox startup, worker startup и total
    cold-start latency p50/p95/p99 до выбора оптимизаций.
  - [ ] Требовать повторную isolation qualification при изменении VMM,
    snapshotter, virtio-fs/shared FS, caching, host daemon privileges или Kata
    annotations.
  - [ ] Вести Dragonball/Nydus/Firecracker как отдельные benchmark и
    qualification tracks; QEMU остаётся первым baseline.

Критерий готовности: удаление или поломка RuntimeClass приводит к отказу до
исполнения пользовательского кода, а не к запуску через `runc`.

### Этап 7. Kubernetes security baseline

Цель: минимально безопасная multi-tenant установка.

- [ ] Отдельный namespace или другой документированный isolation boundary для
  tenant/queue.
  - [ ] Отдельный ServiceAccount для controller и workloads.
  - [ ] У workload Pod и ServiceAccount установить
    `automountServiceAccountToken: false`; разрешён только отдельный Pod-bound
    projected token для ingestion audience.
  - [ ] Workload ServiceAccount не имеет Kubernetes RBAC, а NetworkPolicy
    блокирует доступ sandbox к API server.
  - [ ] Минимальный RBAC без прав на произвольные secrets и cluster resources.
  - [ ] Выдать controller доступ к `pods/log` только в dedicated managed workload
    namespaces; не считать labels RBAC authorization boundary.
  - [ ] Pod Security Admission в режиме `restricted`, если профиль совместим с
    выбранной конфигурацией Kata.
  - [ ] `runAsNonRoot` и фиксированный UID/GID.
  - [ ] `allowPrivilegeEscalation: false`.
  - [ ] Drop всех Linux capabilities.
  - [ ] `seccompProfile: RuntimeDefault`.
  - [ ] Read-only root filesystem и ограниченные writable volumes.
  - [ ] Запрет privileged, hostPID, hostIPC, hostNetwork, hostPath и devices.
  - [ ] Default-deny ingress/egress NetworkPolicy.
  - [ ] Явные allow rules только для ingestion, model gateway, dependency proxies
    и artifact endpoint; прямой доступ sandbox к Git запрещён.
  - [ ] ResourceQuota, LimitRange и ограничение количества активных Jobs/Pods.
  - [ ] Admission policy, запрещающая изменение обязательных sandbox полей.
  - [ ] Image allowlist и обязательные digest references.
  - [ ] Admission verification подписей workload/control-plane images.

Критерий готовности: security test suite не может получить host access,
service-account token с лишними правами, Kubernetes API access или неразрешённый
egress.

### Этап 8. Секреты и credential proxy

Цель: не передавать долгоживущие credentials в JobSpec и по возможности внутрь
sandbox.

- [ ] Ввести opaque `secret_ref` в protocol.
  - [ ] Ограничить разрешённые типы и области секретов серверной политикой.
  - [ ] Реализовать short-lived credentials там, где это возможно.
  - [ ] Реализовать model gateway с controlled TLS termination, provider/model/
    method/path policy, quota и upstream credential injection.
  - [ ] Запретить gateway произвольные upstream URL, private/link-local адреса,
    unapproved redirects, generic tunnelling и unbounded bodies; добавить SSRF и
    exfiltration tests.
  - [ ] Писать authoritative provider usage из model gateway в append-only billing
    ledger по attempt token; worker-reported usage использовать только для UX.
  - [ ] Реализовать dependency proxies для поддерживаемых npm/pub/Cargo/PyPI
    источников и запретить shared writable cache.
  - [ ] Проверять ecosystem integrity/lock metadata, хранить immutable
    content-addressed blobs, изолировать private packages per tenant и запретить
    workload uploads в dependency proxies.
  - [ ] Реализовать artifact endpoint с job-scoped object operations.
  - [ ] Реализовать trusted Git publisher; Git credentials не попадают в sandbox.
  - [ ] Добавить domain/method/path allowlists и default-deny egress policy.
  - [ ] Не считать generic HTTPS `CONNECT` proxy механизмом credential injection.
  - [ ] Исключить секреты из logs, events, diagnostics и metrics labels.
  - [ ] Добавить rotation и revocation workflow.
  - [ ] Получать long-lived trusted-service secrets через external secret manager
    и KMS; не хранить их в ConfigMap/env, аудитировать доступ.
  - [ ] Добавить negative tests на утечки через stdout, stderr и artifacts.

Критерий готовности: компрометация workload не раскрывает долгоживущий ключ
провайдера или control-plane credential.

### Этап 9. Helm и эксплуатация

Цель: воспроизводимая установка и обновление Enterprise Edition.

- [ ] Helm chart для controller, worker configuration и policies.
  - [ ] `values.schema.json` и строгая валидация values.
  - [ ] Pre-install/pre-upgrade checks.
  - [ ] Проверка CRD/API/runtime prerequisites без неявной установки Kata.
  - [ ] Документированный процесс подготовки sandbox node pool.
  - [ ] Поддержка external secret stores через adapters.
  - [ ] PodDisruptionBudget и topology spread для controller.
  - [ ] Helm validation подтверждает физическое отсутствие workload Job PDB во
    всех values combinations.
  - [ ] Metrics, structured logs, traces и health endpoints.
  - [ ] Dashboards и alert rules.
  - [ ] Оповещение о sandbox preflight failure и автоматическое снятие sandbox
    capability.
  - [ ] Global/tenant/queue/capability kill switch для остановки новых leases;
    отмена running jobs остаётся отдельным явно авторизованным действием.
  - [ ] Periodic orphan sweep и cleanup stuck Jobs/Pods/finalization/deletion.
  - [ ] Controller остаётся stateless/recoverable; локальное состояние не является
    authoritative и не требует отдельного backup contract.
  - [ ] Backup/restore и failover tests для canonical PostgreSQL attempt registry;
    start permit не выдаётся из read replica при потере primary.
  - [ ] Upgrade, rollback и compatibility tests для N-1 версии.
  - [ ] Air-gapped installation bundle.

Критерий готовности: чистая установка, upgrade и rollback проходят на каждой
поддерживаемой Kubernetes версии.

### Этап 10. Multi-tenancy и коммерческие функции

Цель: подготовить продукт к нескольким организациям и enterprise operations.

- [ ] Tenant identity во всех jobs, events и audit records.
  - [ ] Tenant-specific quotas и concurrency limits.
  - [ ] Tenant-specific image/capability/network policies.
  - [ ] Спроектировать per-tenant FIFO и weighted deficit round robin с hard
    concurrency limits, bounded weights и защитой от starvation.
  - [ ] Зафиксировать work conservation: единственный активный tenant получает
    всю свободную ёмкость в пределах hard limit, а при появлении других tenants
    capacity честно перераспределяется.
  - [ ] Ввести per-tenant rate/byte/concurrency limits и queue isolation для
    ingestion, object storage/artifact endpoint, model gateway и dependency
    proxies.
  - [ ] Применять Kubernetes PriorityClass только после control-plane fairness, а
    не вместо tenant scheduler policy.
  - [ ] Dedicated node pools как отдельная policy/тарифная возможность.
  - [ ] Usage metering без хранения пользовательского кода и секретов.
  - [ ] Cost attribution по tenant/job/toolchain.
  - [ ] SSO/RBAC интеграция control plane.
  - [ ] Export audit log и configurable retention.
  - [ ] Support bundle с обязательным redaction.

Критерий готовности: действия и ресурсы одного tenant не доступны другому, а
нагрузка одного tenant не обходит установленные квоты.

### Этап 11. Release readiness

Цель: выпустить первый поддерживаемый коммерческий релиз.

- [ ] Обновить ранний `SECURITY_MODEL.md`, закрыть residual findings и провести
  release security review; threat model уже является gate до M3.
  - [ ] Dependency and container vulnerability review.
  - [ ] Software Bill of Materials для каждой поставки.
  - [ ] Подпись images, charts и release manifest.
  - [ ] Performance и soak tests.
  - [ ] Нагрузочные тесты fairness, queue latency и starvation уже на M3–M4, до
    включения production multi-tenancy.
  - [ ] Failure injection: API outage, node loss, eviction, controller restart,
    log/storage outage и exhausted quota.
  - [ ] Документация установки, эксплуатации и troubleshooting.
  - [ ] Runbooks для incident response и emergency disable.
  - [ ] SLA/SLO и support escalation process.
  - [ ] Release notes и migration guide.
  - [ ] Legal review EULA, Apache-2.0 NOTICE и third-party notices.
  - [ ] Пилот минимум с одним design partner.

Критерий готовности: релиз проходит security, reliability, upgrade и support
checklists; известные ограничения опубликованы.

## 9. Helm package: минимальный состав

Chart должен управлять:

- enterprise controller Deployment;
  - ServiceAccount, Role/ClusterRole только при необходимости и bindings;
  - ConfigMap и ссылки на внешние secrets;
  - NetworkPolicies;
  - ResourceQuota и LimitRange templates;
  - PodDisruptionBudget только для controller Deployment, не для workload Jobs;
  - chart не содержит workload PDB template или values flag;
  - metrics Service/ServiceMonitor как опцию;
  - admission policies как отдельную явно включаемую часть;
  - тестовый connection/preflight Job.

Chart не должен молча устанавливать или изменять container runtime на nodes.
Установка containerd/Kata и подготовка node pool оформляются как отдельная
cluster prerequisite или отдельно поддерживаемый infrastructure package.

## 10. Стратегия тестирования

### Unit tests

- protocol serialization и backward compatibility;
  - capability matching;
  - policy validation;
  - Kubernetes manifest generation;
  - status/failure mapping;
  - retry и idempotency logic;
  - event envelope, sequence/ACK, deduplication и semantic validation;
  - deadline ordering и rejection некорректного finalization budget;
  - object manifest commit, uncommitted-prefix GC и fenced publication lease;
  - scoped token authorization;
  - ephemeral-storage manifest mapping;
  - redaction.

### Contract tests

- общий lifecycle для Docker и Kubernetes;
  - success/failure/timeout/cancellation;
  - log ordering и truncation;
  - artifact limits;
  - mandatory verifiers;
  - start permit и attempt/Job identity;
  - finalizing/result_durable state transitions;
  - idempotent completion отдельно от idempotent Git publication;
  - cleanup после частично созданного job.

### Integration tests

- Kubernetes API через локальный test cluster;
  - reconnect watch после потери соединения;
  - controller restart во время Job;
  - ingestion restart и durable queue outage;
  - object storage, workspace-preparer, publisher и purpose-specific egress;
  - Helm install/upgrade/rollback.

### Security tests

- privileged container request;
  - host namespaces и hostPath;
  - попытка изменить RuntimeClass;
  - доступ к Kubernetes API/service-account token;
  - запрещённый egress и DNS tunneling baseline;
  - fork bomb и исчерпание PID/memory/disk;
  - попытка forged worker events/result/version/isolation/verifier claims;
  - попытка execution token list/read/write вне разрешённого attempt/prefix;
  - forged Pod UID, bootstrap replay после удаления Pod и неверный owner Job;
  - model gateway SSRF, private/link-local redirect, quota и oversized bodies;
  - symlink/hardlink/path traversal в workspace и artifacts;
  - shared writable cache poisoning;
  - секреты в logs, diagnostics и artifacts;
  - malicious image и неподписанный image;
  - workload ServiceAccount automount и Kubernetes API reachability;
  - namespace-scoped `pods/log` RBAC без доступа к посторонним namespace;
  - cross-tenant access.

### Reliability tests

- node termination и eviction;
  - Kubernetes API outage;
  - недоступность control plane, ingestion, durable queue, object storage или
    purpose-specific proxy;
  - duplicate/out-of-order delivery, ACK loss и WAL exhaustion;
  - `resync_required`, replay from highest contiguous ACK и missing WAL record;
  - гонка двух ingestion replicas за один start permit;
  - duplicate completion с тем же digest и conflict с другим digest;
  - duplicate Git publish, когда ref уже указывает на intended SHA;
  - Git publish conflict, когда remote ref изменился;
  - два Pod для одного Job: start permit получает только один;
  - `DisruptionTarget`, replacement Pod, permit denial, fast exit и ожидаемый
    `podFailurePolicy` outcome;
  - user-code deadline и finalization grace дают разные failure outcomes;
  - upload без committed manifest удаляется lifecycle GC;
  - terminal ACK loss восстанавливается по committed manifest и server evidence;
  - permit, partial artifact upload, eviction и новый attempt с независимым
    object prefix;
  - два publisher-а одной ref и отклонение stale fencing token;
  - зависший Pod и зависшее удаление;
  - stuck finalization, orphan sweep и cleanup deadline;
  - global/tenant/queue kill switch без неявной отмены running jobs;
  - fairness и starvation при конкурирующих tenant queues;
  - work-conserving scheduling при одном активном tenant и fair reclamation при
    появлении второго;
  - исчерпание quota;
  - длительные soak tests с контролем утечек ресурсов.

## 11. Observability и SLO

Минимальные метрики:

- leased/running/completed/failed/cancelled jobs;
  - queue wait и execution duration;
  - scheduling latency;
  - sandbox startup latency;
  - infrastructure failure rate;
  - log/artifact upload failures;
  - cleanup backlog;
  - active Pods по bounded operational dimensions без tenant/job identifiers;
  - rejected jobs по policy reason;
  - preflight/runtime health;
  - агрегированные CPU, memory и ephemeral-storage usage.

Prometheus не получает `job_id`, `lease_id`, `run_id`, `execution_id` или
`tenant_id` в labels. Kubernetes object labels для watch/orphan reconciliation
не экспортируются в метрики автоматически.

Structured logs и traces содержат полную correlation identity (`job_id`,
attempt, lease, run, execution и tenant) под отдельными access/retention
policies. Audit/event store хранит canonical lifecycle и security decisions.
Usage и cost attribution записываются в append-only billing ledger в
реляционном или аналитическом хранилище; billing не восстанавливается из
Prometheus. Provider usage для биллинга поступает от model gateway по attempt
identity; worker-reported usage используется только для UX. Коррекция создаёт
compensating record.

Ни один telemetry channel не содержит секреты, команды с credentials или
содержимое пользовательского кода по умолчанию.

Предварительные SLO определяются после нагрузочного пилота. До появления данных
не следует публиковать численные SLA.

## 12. Риски и способы снижения

| Риск | Снижение |
| --- | --- |
| Core API нестабилен и ломает private repository | Versioned protocol, semver, fixtures и Enterprise CI на каждую новую public версию |
| Job выполняется повторно | Idempotency keys, immutable attempt identity, controlled Git publication и deduplicated completion |
| Pod/node потерян до чтения логов | Outbound durable ingestion, bounded WAL, object storage и Kubernetes logs только как fallback |
| Worker подделывает сообщения | Scoped execution token, bounded schema/sequence validation и server-owned canonical state machine |
| Result загружен, но ACK потерян | Digest-keyed idempotent write и replay того же terminal result |
| Blob загружен без terminal commit | Immutable manifest-as-commit, per-run prefix и lifecycle GC uncommitted objects |
| Две control-plane replicas выдают permit | PostgreSQL-primary transaction/CAS; cache, memory и read replica не являются authority |
| Два publisher-а меняют одну ref | Durable per-ref fenced lease плюс Git remote compare-and-set |
| Shared cache отравлен | Запрет shared writable cache, content-addressed read-only objects и trusted builders/proxies |
| Kata настроен неверно | Startup preflight, runtime canary, dedicated nodes и fail-closed leasing |
| Оптимизация shared FS ослабляет isolation | Любое изменение VMM/shared FS/snapshotter требует новой isolation qualification |
| Секрет попадает в sandbox или лог | `secret_ref`, credential proxy, short-lived tokens, redaction и negative tests |
| Kubernetes-specific логика расползается по core | Executor boundary, manifest builder и запрет `if enterprise` в public workflow engine |
| Service sidecars не завершаются вместе с Job | Отдельный design/PoC; до решения capability `services` не объявляется |
| API server перегружен Jobs и Pods | Watch вместо polling, TTL cleanup, quotas и cleanup monitoring |
| Несовместимость Kubernetes/Kata версий | Узкая опубликованная support matrix и qualification suite |
| Apache-2.0 attribution нарушен | Automated NOTICE/SBOM generation и release legal checklist |

## 13. Предлагаемый порядок первых pull requests

### Public repository

1. ADR о Community/Enterprise boundary.
   2. ADR 0003 о durable delivery и Enterprise trust boundaries.
   3. ADR 0004 и ранний `SECURITY_MODEL.md` с secure-delivery gates.
   4. `ExecutorCapabilities`, `DoctorReport` и conformance skeleton.
   5. `ExecutorFactory`; удалить прямые ссылки на Docker из daemon.
   6. Разделить общую и Docker-specific конфигурацию.
   7. Cargo workspace и `runner-protocol`.
   8. `runner-core`, `executor-docker`, `runner-cli`.
   9. Job protocol `v1beta1`, resource/storage contract и compatibility fixtures.
   10. Heartbeat, cancellation, event identity и idempotent completion.
   11. `secret_ref` и scoped job-token contracts.

### Private enterprise repository

1. Repository skeleton, CI, licensing и dependency pinning.
   2. Kubernetes manifest builder и unit tests.
   3. Event ingestion, durable store adapter и object layout.
   4. PostgreSQL-primary start-permit PoC и projected bootstrap-token validation.
   5. Create/watch/log-fallback/delete для простого Job.
   6. Start permit, recovery, orphan reconciliation и idempotency.
   7. Runner worker, trusted workspace preparer/publisher и artifact transport.
   8. Kata RuntimeClass enforcement и preflight.
   9. Security policies и negative tests.
   10. Helm lifecycle и operational kill switches.
   11. Model gateway, dependency proxies и artifact endpoint.
   12. Pilot release.

## 14. Ориентировочные этапы поставки

Оценка предполагает двух основных разработчиков — Rust/backend и
Kubernetes/platform — с регулярным участием security-инженера.

| Milestone | Результат | Ориентир |
| --- | --- | --- |
| M0 | ADR, threat model и утверждённая граница продукта | 1 неделя |
| M1 | Разделённый public core и executor factory | 2–3 недели |
| M2 | Стабильный protocol, delivery identity и control-plane contract | 2–3 недели |
| M3 | Kubernetes Job MVP, ingestion и durable result path | 3–5 недель |
| M4 | Kata, security baseline и Helm | 3–5 недель |
| M5 | Credential proxy, observability и pilot hardening | 3–6 недель |
| M6 | Первый commercial release candidate | после успешного пилота |

Эти интервалы не складываются механически: часть работ может выполняться
параллельно после стабилизации public protocol. Первая оценка уточняется после
M1 и Kubernetes/Kata proof of concept.

## 15. Ближайший рабочий backlog

Следующие задачи считаются непосредственным стартом разработки:

- [x] Создать ADR `0002-community-docker-enterprise-kubernetes.md`.
- [x] Создать ADR `0003-enterprise-delivery-and-trust-boundaries.md`.
- [x] Создать ADR `0004-enterprise-threat-model-and-secure-delivery-gates.md`.
- [x] Создать и первично заполнить `SECURITY_MODEL.md`.
- [ ] Провести initial security review `SECURITY_MODEL.md` перед Enterprise M3.
- [x] Спроектировать `ExecutorCapabilities` и `ExecutionRequirements`.
- [x] Добавить `ExecutorFactory`.
- [x] Перевести daemon на `Arc<dyn Executor>`.
- [x] Перевести `doctor`, `run` и `cleanup` на выбранный executor.
- [ ] Удалить проверку `executor == "docker"` из общего JobSpec validator и
  перенести совместимость в capability policy.
- [x] Добавить mock executor и первые conformance tests.
- [x] Составить и выполнить Cargo workspace migration plan.
- [x] Утвердить Kubernetes/Kata qualification baseline для первого PoC.
- [ ] Создать минимальный private enterprise repository после стабилизации
  executor factory.

## 16. Definition of Done коммерческой версии 1.0

Enterprise 1.0 считается готовой, когда:

- sandboxed job невозможно запустить без подтверждённого Kata runtime;
- controller восстанавливает наблюдение после рестарта;
- canonical events и result переживают рестарт controller и недоступность
  ingestion/storage в пределах bounded retry/finalization policy;
- только один Pod получает start permit для одного attempt;
- start permit и publisher lease выдаются только через linearizable
  PostgreSQL-primary operations;
- bootstrap identity проверяется через TokenReview и прямую сверку Pod UID/
  owner с Kubernetes API;
- completion и Git publication независимо идемпотентны;
- `completed` недоступен до `result_durable`;
- timeout и cancellation гарантированно останавливают workload;
- cleanup не оставляет неограниченно растущие Jobs, Pods и volumes;
- object lifecycle удаляет uncommitted uploads, а committed manifest допускает
  безопасный recovery после потери terminal ACK;
- tenant quotas и network isolation применяются принудительно;
- долгоживущие model/Git credentials не передаются напрямую в JobSpec;
- workspace готовится и публикуется trusted services вне sandbox;
- Prometheus, audit и append-only billing ledger имеют раздельные контракты;
- operational kill switch останавливает новые leases по заданному scope;
- установка, upgrade и rollback проходят на всей support matrix;
- security и failure-injection suites проходят в CI;
- каждый Enterprise component проходит Definition of Secure Done из
  `SECURITY_MODEL.md`;
- images и charts подписаны и сопровождаются SBOM;
- документация и runbooks проверены в пилотной установке;
- Apache-2.0 LICENSE/NOTICE и third-party attributions включены в поставку;
- известные ограничения и support boundaries опубликованы.
