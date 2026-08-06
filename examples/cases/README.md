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
