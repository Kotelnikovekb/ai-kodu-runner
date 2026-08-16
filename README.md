# ai-codu-runer

`ai-codu-runer` is a small native Rust worker that runs versioned jobs in one-shot Linux containers. On Linux it uses Docker Engine; on macOS Docker Desktop is used through the same Docker Engine API. OpenCode, Node, Flutter and browser tooling belong in prebuilt job images, not in the runner.

## Quick start

Install Rust stable and Docker Desktop/Engine, then:

```sh
cargo build --release
cargo run -- doctor
cargo run -- run --job examples/job.json
cargo test
```

Runnable examples are documented in [`examples/cases/README.md`](examples/cases/README.md): a successful Alpine job, per-job secret injection, an expected failure, and a Flutter/OpenCode verifier template.

Product direction, compatibility, and architectural decisions are documented
in [`docs/ROADMAP.md`](docs/ROADMAP.md), [`docs/SECURITY_MODEL.md`](docs/SECURITY_MODEL.md),
[`docs/SUPPORT.md`](docs/SUPPORT.md), [`docs/VERSIONING.md`](docs/VERSIONING.md), and
[`docs/decisions/`](docs/decisions/). In particular, ADR 0002 defines the
Community/Enterprise boundary, ADR 0003 defines durable Enterprise delivery and
trust boundaries, and ADR 0004 makes the threat model an implementation gate.

The local example uses the current directory as a workspace and `network: none`. For `workspace.kind = local`, the path must be inside configured `work_dir`. `archive_url` accepts HTTPS tar archives only; absolute paths, `..`, symlinks and hardlinks are rejected.

Git workspaces clone and check out the requested `branch`, then fetch
`base_branch` so agents can inspect the implementation diff. The optional
`head_sha` and `base_sha` fields make checkout validation immutable: the job
fails during preparation if either fetched revision differs. `publish_mode`
controls runner-managed commits and accepts `disabled`, `if_changed` (the
backward-compatible default), or `required`. A clean `if_changed` workspace is
a successful no-op; a clean `required` workspace fails explicitly.

## Commands

```text
ai-codu-runer doctor
ai-codu-runer run --job ./job.json [--config ./runner.toml]
ai-codu-runer daemon --config ./runner.toml
ai-codu-runer cleanup --config ./runner.toml
ai-codu-runer version
```

`doctor` pings Docker, reports host/API/capabilities, and creates/removes a disposable Alpine container. The daemon leases jobs from the HTTP control plane, sends Bearer-authenticated completion requests, and stops cleanly on Ctrl-C. Docker container architecture is configured with `[docker].platform` and defaults to `linux/amd64`; `RUNNER_PLATFORM` overrides it for local or host-specific deployments. Values must use `os/architecture` format, optionally with a variant such as `linux/arm64/v8`.

## Architecture and safety

The Cargo workspace is split into `runner-protocol`, `runner-core`, and
`executor-docker`; the root package keeps the Community CLI, daemon and HTTP
adapter. `Executor` and `ControlPlane` are the extension points for future
backends. Before execution, the factory admits a job only when the selected
executor satisfies its isolation and capability requirements; the Community
Docker executor supports container isolation only.

Every job gets a temporary copied workspace, managed labels, a private network when requested, a read-only root filesystem, `/tmp` tmpfs, dropped capabilities, `no-new-privileges`, CPU/memory/PID limits, bounded logs, and a timeout. The runner never accepts a Docker `HostConfig` from the server. It does not mount the host Docker socket, use privileged mode, host network/PID/IPC, arbitrary devices, or arbitrary host mounts. Only configured environment variable names may cross the boundary, and values are never written to the SQLite journal.

Workspace staging prunes generated directories named `.cache`, `.omniroute`,
`.dart_tool`, `.runner-cache`, `build`, and `node_modules` at any depth. They must
be recreated by setup commands or the toolchain inside the isolated job. This
keeps dependency caches and OpenCode state out of source snapshots.

The default root filesystem is read-only. The Docker executor automatically
provides ephemeral writable tmpfs mounts for `/tmp` and the standard
`/home/opencode` config, state, data, npm, and pub-cache directories. A job may
still explicitly set `"writable_rootfs": true` when its image requires writes
outside those paths, for example when Flutter updates its SDK cache under
`/usr/local/flutter/bin/cache`. Such a relaxation is visible in the JobSpec and
should only be allowed for trusted, pinned images.

Tool images keep `HOME` and all XDG directories under `/home/opencode`, never
under `/workspace`. Project `opencode.json` files disable snapshots for
non-interactive jobs. Downloads that are worth reusing should be baked into the
image or provided by a future runner-managed cache volume; OpenCode session DBs,
logs, locks, and snapshot repositories must not be shared as dependency caches.
The sample one-shot images use `OPENCODE_DB=:memory:`; remove that override only
when a product flow explicitly resumes an OpenCode session from durable storage.

Task-specific secrets can be supplied through `JobSpec.secrets`, for example `{ "name": "OPENAI_API_KEY", "value": "..." }`. The value is held in memory and injected only into that job's container. Known runner-managed values are redacted from failure diagnostics; arbitrary secrets printed by user code cannot be reliably detected. Secret names must be allowed by runner policy and are size-limited. `secret_ref` is part of the protocol for external secret resolvers, but the Community executor rejects it closed because it cannot resolve references safely. Failed results may include a typed `failure` with a stable kind and code. The legacy `environment_from_runner` field remains supported for local secrets supplied by the runner process.

The SQLite journal records the explicit lifecycle `received → preparing → running → collecting → completed|failed|cancelled|timed_out → destroying → destroyed`. Cleanup filters Docker resources by both `omniroute.managed=true` and the local runner ID, so another runner's resources are out of scope. Janitor/cleanup is deliberately conservative; resources are not removed solely because they have a similar name.

For local `run`, set `attempt` to `0` (or omit it). The runner allocates the next
attempt number for that `job_id` from SQLite, so repeated runs create separate
directories such as `artifacts/<job_id>/1`, `artifacts/<job_id>/2`, and so on.
Daemon jobs keep the attempt number supplied by the control plane.

For production, use immutable digest-pinned images in daemon jobs and a control plane that authenticates leases and makes completion idempotent. The HTTP adapter uses bounded connect/request timeouts, renews leases with periodic typed heartbeats, cancels work after an explicit server decision or consecutive heartbeat failures, and sends a stable `Idempotency-Key` derived from `(job_id, attempt)` for bounded completion retries. A completion that exhausts its retry budget is reported as an operational failure; durable pending-completion storage belongs to the Enterprise control plane.

### Agent + verifier workflow

`JobSpec.workflow` is the generic feedback loop. The runner does not know whether the
workspace contains PHP, Flutter, React, Rust or anything else. It executes only the
argv arrays declared by the job:

```json
"workflow": {
  "setup": [{"command": ["git", "clone", "https://git.example/app.git", "/workspace/app"]}],
  "agent": {
    "command": ["opencode", "run", "--format", "json", "-f", "/workspace/prompt.md"]
  },
  "verifiers": [
    {"name": "tests", "command": ["./ci/run-tests"], "required": true},
    {"name": "security", "command": ["trivy", "fs", "--exit-code", "1", "."], "required": true}
  ],
  "max_iterations": 3,
  "feedback_file": "/workspace/.runner/feedback.md"
}
```

The agent runs first, then verifiers run independently. Failed verifier output is
written to `feedback_file`; the next agent iteration receives that file through the
job's command. A job is completed only when all required verifiers pass. An optional
`workflow.publish` argv command runs after all checks pass.

Workflow jobs may also declare disposable backend services. Services are started
on the job's private Docker network before `setup`, and are removed after artifact
collection. They never publish ports on the host. A service healthcheck is an argv
array executed from the runner until it succeeds:

```json
"services": [
  {
    "name": "postgres",
    "image": "postgres:16@sha256:...",
    "alias": "db",
    "environment": {
      "POSTGRES_DB": "test",
      "POSTGRES_USER": "test",
      "POSTGRES_PASSWORD": "test"
    },
    "healthcheck": {
      "command": ["pg_isready", "-U", "test"],
      "timeout_seconds": 60
    }
  },
  {
    "name": "redis",
    "image": "redis:7@sha256:...",
    "alias": "redis",
    "healthcheck": { "command": ["redis-cli", "ping"] }
  }
]
```

Services require `network.mode = "bridge"`; the application reaches them by
their aliases (`db`, `redis`, etc.). Runner-level mandatory verifiers can be
configured under `[security].mandatory_verifiers` in `runner.toml`. They run in
addition to job verifiers and cannot be omitted by a JobSpec.

### Browser and device testing

Browser tests are supported by putting Playwright or Cypress and the required
browser binaries into the job image, then declaring the test command as a
verifier. A browser service can also be started as a service container when the
application is tested over the private job network.

An Android emulator is different: it normally needs KVM and `/dev/kvm`, a
privileged or specially configured container, and often a nested-virtualization
capable host. The current runner deliberately does not expose devices or allow
privileged containers, so it should not run Android emulators inside ordinary
jobs. The safe extension is a separate device executor/worker with an explicit
capability such as `android-emulator`, isolated device allocation, timeout and
cleanup. The job then calls that worker or uses an external device farm. iOS
simulators require a macOS worker and cannot be provided by a Linux Docker
container.

Artifact export is bounded by both `limits.max_artifact_bytes` and
`limits.max_artifact_files`. Broad patterns such as `"**"` do not descend into
generated/cache directories. A precise prefix such as `"build/**"` can opt in to
a required generated tree. Prefer narrow source and report patterns so collection
does not duplicate dependency caches after every job.

Workspace preparation is bounded by `limits.max_workspace_bytes`,
`limits.max_workspace_files`, and (for HTTPS tar archives)
`limits.max_archive_download_bytes`. Archive traversal, links and device/FIFO
entries are rejected. Preparation also observes the job cancellation token and
deadline, so a stalled Git or archive source cannot run outside the job budget.
Failure diagnostics contain only bounded, known-secret-redacted stdout/stderr;
prompt, `AGENTS.md`, and feedback files are never copied into diagnostics.

Git and merge requests remain provider-neutral: use `setup` for clone/fetch/checkout
and `publish` for push or a provider CLI such as `glab mr create`/`gh pr create`.
Credentials must be supplied as per-job secrets or runner-approved environment names;
never put tokens in clone URLs or prompts. The image must contain the selected Git or
provider CLI. This keeps the Rust runner independent of GitLab, GitHub and project
languages.

### Flutter verifier job

`examples/flutter-agent-job-local.json` is the runnable local version of that
workflow. The image must contain Flutter, Dart, OpenCode, Node/npm and the verifier
CLIs. Its setup phase installs the official Flutter and Dart skills into the project
for OpenCode. The agent then creates or updates `AGENTS.md` itself; this is used
instead of relying on the interactive `/init` TUI command.

Verify the image before running the job:

```bash
docker run --rm flutter-opencode:local \
  sh -lc 'flutter --version && dart --version && opencode --version && node --version'
```

The workflow also has an explicit optional `initialize` phase. It runs a separate
non-interactive `opencode run` before coding and is responsible only for creating or
updating `AGENTS.md`; the main agent then receives that context.

The project OpenCode config enables Dart LSP with `"lsp": true`. The Flutter image
already provides the `dart` executable, so OpenCode can start its built-in Dart
server when it opens `.dart` files. LSP assists with diagnostics and navigation;
the authoritative acceptance checks remain `flutter analyze`, formatting, tests
and builds.

The Context7 MCP configuration is project-local OpenCode configuration; keep its key
out of Git and inject it through `JobSpec.secrets` or approved runner environment
names. The remote MCP endpoint requires bridge networking.

## Development checks

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The unit tests cover policy clamping, lifecycle transitions and archive/path safety. Docker-dependent checks should be run when Docker is available and are expected to skip in CI environments without a daemon.

## License and third-party components

This project is licensed under the Apache License, Version 2.0. See
[`LICENSE.md`](LICENSE.md). The main third-party components and tool-image
dependencies are listed in [`docs/THIRD_PARTY_NOTICES.md`](docs/THIRD_PARTY_NOTICES.md).

The published Docker images contain software from their base images and package
managers. Their exact dependency inventory may change with pinned runtime and
base-image versions; releases should be accompanied by the CI-generated SBOM.
