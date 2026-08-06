# Flutter job image contract

The Rust runner does not install project tooling. Build or publish an immutable Docker image containing the tools required by the job:

Required:

- Flutter SDK and Dart SDK;
- OpenCode CLI;
- Node/npm if OpenCode or project tooling requires it;
- a non-root user that can write `/workspace`;
- CA certificates and Git if dependencies need to be fetched.

Optional verifier tools:

- Playwright and browser binaries for web UI tests;
- Maestro plus an Android/iOS-capable execution environment for mobile UI tests;
- Trivy for filesystem/dependency/secret/IaC scans.

The job treats Flutter checks as required. Playwright, Maestro and Trivy are skipped when the project or executable is absent. This keeps the same job usable for projects that do not include those verifier profiles.

The archive used by `examples/flutter-job.json` should contain:

```text
prompt.md
opencode.json
.opencode/skills/*/SKILL.md
pubspec.yaml
lib/
test/
integration_test/       # optional
web/                    # optional
playwright.config.*     # optional
.maestro/               # optional
Dockerfile              # enables Trivy check
```
