# AGENTS.md

This file applies to the entire repository. It is the working agreement for
coding agents and contributors making changes to AI Kodu Runner.

## Repository purpose

This repository is the Apache-2.0 Community Edition of AI Kodu Runner. It runs
versioned jobs in one-shot Docker containers and contains the public protocol,
workflow, verifier, workspace, artifact, CLI, and Docker-executor code.

Kubernetes, Kata, Helm, multi-tenant control-plane services, credential
gateways, and other Enterprise implementation code belong in the separate
private Enterprise repository. Public contracts, architecture records,
security requirements, and compatibility fixtures that both editions need may
live here.

Do not add customer data, private repository source, commercial credentials,
private Helm charts, production endpoints, or confidential vulnerability
details to this repository.

## Read before changing code

Read the files relevant to the task:

- `README.md` for current behavior and local usage;
- `docs/GLOSSARY.md` for terminology and key concepts used throughout the codebase;
- `CONTRIBUTING.md` for contribution and DCO requirements;
- `SECURITY.md` and `docs/SECURITY_MODEL.md` for security boundaries;
- `docs/VERSIONING.md` for public API, JobSpec, and worker-protocol compatibility;
- `docs/SUPPORT.md` for the qualified Enterprise runtime baseline;
- `docs/ROADMAP.md` for planned work, not necessarily implemented behavior;
- `docs/decisions/` for accepted architecture decisions.

ADR 0001 defines the agent/verifier and Git-workspace behavior. ADR 0002
defines the Community/Enterprise boundary. ADR 0003 defines durable Enterprise
delivery and trust boundaries. ADR 0004 makes threat modelling an early
Enterprise implementation gate.

Architecture Decision Records in `docs/decisions/` capture the project's accumulated
design wisdom: what was tried, what was rejected, why current patterns exist,
and under what constraints decisions were made. When planning new features or
debugging unexpected behavior, read relevant ADRs first to avoid repeating
already-explored dead ends or unknowingly violating documented invariants.

If code and roadmap differ, treat the code and tests as current behavior and
the roadmap as future intent. Do not silently implement a roadmap item as part
of an unrelated change.

## Current project shape

The repository is a Rust 2024 Cargo workspace. The public Community binary
uses the root package `ai-kodu-runner`; shared code is split into
protocol, core, and Docker executor crates. A separate `runner-cli` crate is
still deferred.

Key modules:

- `crates/runner-protocol`: versioned JobSpec, JobResult, workflow and fixtures;
- `crates/runner-core`: policy, config, workspace, artifacts, journal/state and
  executor contracts;
- `crates/executor-docker`: Docker-specific execution, workflow services and
  cleanup;
- `crates/runner-core/tests/community_conformance.rs`: public executor
  conformance checks;
- `src/control/`: HTTP daemon control-plane adapter;
- `src/main.rs`, `src/cli.rs`, and compatibility facades: Community CLI and
  root binary wiring.

The package and binary use the public name `ai-kodu-runner`.
Renaming it is a compatibility and release change; do not correct the spelling
incidentally.

## Development workflow

Keep changes focused and preserve unrelated user modifications in a dirty
worktree. Prefer extending existing modules and patterns over introducing a new
abstraction without a concrete need.

Before implementing features or investigating bugs:

- **Study `docs/decisions/`** for relevant architecture decisions and rationale. ADRs
  capture hard-won lessons, rejected alternatives, and context for existing
  design. Do not repeat already-considered approaches without understanding why
  they were rejected.
- **Check `docs/GLOSSARY.md`** to ensure consistent use of established terminology.
  If your feature introduces new concepts, technical terms, or domain-specific
  vocabulary that will be referenced across multiple files, add clear
  definitions to `docs/GLOSSARY.md` as part of the change.

For Rust changes:

- use stable, idiomatic Rust compatible with edition 2024;
- keep async blocking work out of Tokio executor threads;
- prefer typed errors and explicit failure mapping over string inspection;
- keep user-supplied identifiers, paths, URLs, resource values, and output
  bounded and validated at the boundary;
- never log secret values or include them in journal/result/debug structures;
- add focused unit tests beside existing tests for changed behavior;
- update protocol fixtures and compatibility documentation when serialized
  behavior changes;
- avoid adding a dependency when the standard library or an existing
  dependency is sufficient.

Do not make broad formatting, naming, dependency, or module-layout changes in a
feature or bug-fix patch unless they are required by the task.

## Required checks

Run checks proportional to the change. The full local/CI baseline is:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

Documentation-only changes should at least be checked for broken local links,
invalid paths, and formatting whitespace. Docker-dependent behavior should be
tested when a Docker daemon is available; clearly report checks that could not
run.

Never claim a check passed unless it was executed. Report the exact failing
command and relevant error when the failure is environmental or pre-existing.

## Architecture boundaries

- Common workflow and protocol code must not depend on Docker or Kubernetes.
- Executor selection belongs in the factory/control-plane policy, not scattered
  `if enterprise` or executor-name checks.
- Jobs declare execution requirements and capabilities; clients do not select
  an arbitrary privileged backend configuration.
- Docker-specific behavior stays behind the executor boundary.
- Public core must not accept arbitrary Docker `HostConfig`, Kubernetes
  PodSpec, host mount, device, namespace, ServiceAccount, or runtime class from
  JobSpec.
- A requirement for `sandboxed` execution must fail closed on an executor that
  cannot provide it. Never silently downgrade Kata or another required sandbox
  to `runc`/plain Docker.
- Completion, result storage, artifact upload, and Git publication are separate
  idempotency domains.
- Worker messages are authenticated evidence, not canonical authority.

Changes that alter a public Rust API, serialized protocol, state transition,
security boundary, executor contract, publication behavior, or compatibility
policy require tests, documentation updates, and usually an ADR update or a new
ADR.

## Security invariants

Preserve these invariants even in prototypes:

- no long-lived provider, Git, object-store master, or control-plane credential
  enters an untrusted workload;
- secrets do not appear in logs, errors, artifacts, fixtures, screenshots, or
  support bundles;
- archive handling rejects absolute paths, traversal, device entries, unsafe
  symlinks/hardlinks, excessive file counts, excessive expanded size, and
  decompression bombs;
- Git preparation retains exact `head_sha` and `base_sha` when required;
  unconditional `--depth 1` must not break diff-based verification;
- cleanup deletes only resources carrying all required product ownership
  markers; similar names alone are insufficient;
- production images are immutable and digest-pinned;
- resource, log, event, WAL, workspace, and artifact limits are explicit and
  fail closed;
- high-cardinality job, run, execution, lease, and tenant identifiers do not
  become Prometheus labels;
- Community Docker execution is not described as hostile multi-tenant
  isolation.

For Enterprise contract or documentation work, additionally preserve the
authorities and threat model in `docs/SECURITY_MODEL.md`: PostgreSQL-primary permit
CAS, exact Pod UID verification, projected bootstrap identity, durable event
ACK, immutable object manifests, fenced publication leases, purpose-specific
egress, and no workload Kubernetes API access.

## Tests and fixtures

Test externally observable behavior and boundary failures, not private helper
implementation details. Include negative cases for malformed or adversarial
input whenever validation changes.

For protocol changes, cover:

- current serialization;
- supported older fixtures;
- unknown/additive fields where applicable;
- invalid capability/resource combinations;
- idempotent replay and conflicting identity reuse;
- typed failure semantics.

For workspace or artifact changes, cover traversal, symlink/hardlink, size,
count, redaction, and partial-failure cleanup paths.

## Documentation and licensing

Keep terminology consistent across `README.md`, `docs/ROADMAP.md`, ADRs,
`docs/SECURITY_MODEL.md`, `docs/SUPPORT.md`, `docs/VERSIONING.md`, and
`docs/GLOSSARY.md`. Use Community for the public Docker edition and Enterprise
for the private Kubernetes/Kata edition.

When implementing features that introduce new technical concepts, architectural
components, protocol elements, or domain-specific terminology:

1. Add clear definitions to `docs/GLOSSARY.md` using existing entries as a template.
2. Include context: what it is, why it exists, how it relates to other concepts.
3. Reference the glossary entry from relevant documentation files.
4. Update the glossary version date when making significant additions.

The glossary serves as a single source of truth for terminology used in code,
documentation, ADRs, and communication with contributors. It reduces ambiguity
and helps maintain consistency as the project evolves.

New source files intended for distribution should carry the repository's
Apache-2.0 header where the surrounding files use it. Preserve `LICENSE.md`,
`NOTICE`, and third-party attribution requirements. Do not copy code from a
source whose license and provenance are unclear.

Security vulnerabilities must be reported privately according to
`SECURITY.md`, not documented in a public issue or pull request before
coordinated disclosure.

## Handoff expectations

At the end of a task, state:

- what changed and why;
- which files define the behavior;
- which checks ran and their result;
- any remaining risk, assumption, migration, or follow-up work.

Do not claim Enterprise-grade isolation, exactly-once execution, durable
delivery, compatibility, or release readiness without the corresponding
implementation and qualification evidence.
