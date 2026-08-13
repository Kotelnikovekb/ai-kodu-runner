# Runnable examples

Run every command from the repository root. Docker Desktop/Engine must be running.

Prepare the small test image once:

```bash
docker pull alpine:3.20
```

## 01 — successful command and artifact

```bash
cargo run -- run --job examples/cases/01-basic/job.json
```

Checks stdout, stderr, exit code `0`, and an artifact.

## 02 — per-job secret

```bash
cargo run -- run \
  --config examples/local-runner.toml \
  --job examples/cases/02-secret/job.json
```

The secret is present only in the JobSpec and is injected into this container. The command checks it without printing its value.

## 03 — expected failure

```bash
cargo run -- run --job examples/cases/03-failure/job.json
```

This intentionally exits with code `7`. The runner should return `status: "failed"` and still destroy the container and temporary workspace.

## 04 — Flutter/OpenCode verifier template

```bash
cargo run -- run --config examples/local-runner.toml --job examples/flutter-agent-job-local.json
```

The local job is runnable after building `flutter-opencode:local`. The image must
contain Flutter, Dart, OpenCode and the verifier CLIs. The job runs OpenCode first,
then Flutter/Dart checks and a web build. For a server job, use the archive variant
and replace the image with an immutable digest plus an HTTPS archive URL.

`examples/flutter-agent-job-local.json` is the local generic agent + verifier example.
It declares the agent and Flutter checks as separate argv arrays in `workflow`; there
is no Flutter-specific logic in the Rust runner and no shell pipeline. Failed checks
are written to `.runner/feedback.md` and supplied to the next agent iteration.

The sample OpenCode config explicitly selects `freemodel/auto`. This prevents an unrelated global OpenCode provider such as a paid Cline/Claude model from being selected. A `402 Insufficient balance` response is not retryable; change the selected model/provider or replenish its balance. Retries are appropriate for temporary network/5xx/429 errors only.

## 05 — large prompt from a file

`examples/cases/04-large-prompt/job.json` demonstrates the important pattern for a multi-megabyte prompt. It uses OpenCode's file attachment option instead of interpolating the prompt into a shell argument:

```text
opencode run --format json -f /workspace/prompt.md "Implement the attached task..."
```

Put `examples/prompt.md` into the same archive as the Flutter project under the archive root as `prompt.md`, then replace the image digest and archive URL in the job. OpenCode reads the file from the workspace; the shell does not copy the prompt into argv.

For a local transfer-only smoke test using the existing `examples/prompt.md`:

```bash
cargo run -- run --job examples/cases/04-large-prompt/job-local.json
```

This uses Alpine, verifies that `/workspace/prompt.md` is non-empty and contains the expected task, and returns `prompt-size.txt` and `prompt-check.txt` as artifacts. It does not run OpenCode or Flutter; use `job.json` with a real Flutter/OpenCode image for that stage.

## 05 — Next.js AI Prompt & Task Studio

```bash
cargo run -- run --job examples/cases/05-nextjs-ai-prompt-chat/job.json
```

This case asks the agent to create a multi-page Next.js prompt/task workspace
with Untitled UI and SQLite persistence. It verifies lint, TypeScript, tests,
production build, React Doctor and Trivy. Build a local
`nextjs-opencode:local` image with Node.js, OpenCode, React Doctor and Trivy.
The mock AI provider keeps the test deterministic and does not require an
external API key. Afrog, RustScan and reverse-skill are intentionally not part
of this default case: they belong in a separately authorized active-security
profile with a running target and explicit scope.

Build the tool image from the repository root:

```bash
docker build \
  -f examples/nextjs-opencode.Dockerfile \
  -t nextjs-opencode:local \
  .
```

Verify the image before running the job:

```bash
docker run --rm nextjs-opencode:local \
  bash -lc 'node --version && npm --version && opencode --version && trivy --version'
```

The agent itself needs an LLM provider key. The case includes an OpenCode config
for FreeModel and reads `FREEMODEL_API_KEY` only from the runner environment:

```bash
export FREEMODEL_API_KEY='your-key'
export FREEMODEL_BASE_URL='https://api.freemodel.dev/v1'
cargo run -- run \
  --config examples/local-runner.toml \
  --job examples/cases/05-nextjs-ai-prompt-chat/job.json
```

The application created by the agent still uses a deterministic mock AI provider;
the key above is used only by OpenCode while implementing the task.

## 06 — Python/FastAPI content-factory MVP

This case runs OpenCode inside a Python 3.12 container and gives the job disposable
PostgreSQL and Redis services. The agent implements the backend described in
`IMPLEMENTATION_CASE.md`, including sources, evidence bundles, article versions,
comments, Celery revisions and quality checks.

Build the image from the repository root:

```bash
docker build \
  -f examples/python-opencode.Dockerfile \
  -t python-opencode:local \
  .
```

The case uses FreeModel for OpenCode. The API key is supplied by the runner
environment and is not stored in `job.json`:

```bash
export FREEMODEL_API_KEY='your-key'
cargo run -- run \
  --config examples/local-runner.toml \
  --job examples/cases/06-python-fastapi-opencode/job.json
```

Application LLM and embedding models are configured by the generated project's
`.env.example` and are not hardcoded in source code. Tests must use fake providers.
