# Versioning and compatibility policy

## Scope

AI Kodu Runner versions three different interfaces independently:

- public Rust crates;
- the serialized runner protocol (`JobSpec`, results, and events);
- Community and Enterprise product releases.

An Enterprise release records all three values in its private release manifest.
It never relies on an unbounded Cargo dependency range for public runner crates.

## Public Rust crates

Public crates use Semantic Versioning.

Before `1.0.0`, all public workspace crates release as one version train:

- a patch release (`0.y.z` to `0.y.z+1`) contains backward-compatible fixes,
  documentation, tests, and additive implementation-only behavior;
- a minor release (`0.y.z` to `0.(y+1).0`) may change a public Rust API,
  protocol adapter, CLI behavior, or feature boundary;
- a new crate begins at the current release-train version and follows the same
  policy.

After `1.0.0`, a public API break requires a major version increase. Additive,
backward-compatible public APIs use a minor version increase; fixes use a patch
version increase.

`runner-protocol`, `runner-core`, `executor-docker`, and `runner-cli` are
released together while they are in the initial workspace migration. They may
be versioned independently only after their compatibility suites prove that a
separate release does not create an unsupported combination.

## Wire protocol

`api_version` is a wire-compatibility contract and is not inferred from the
Cargo crate version.

The current Community HTTP control-plane surface is specified in
[`openapi/community-control-plane.yaml`](openapi/community-control-plane.yaml).
The canonical JSON shapes are kept in [`schemas/`](schemas/), including the
stable `FailureInfo` envelope in `failure-info-v1.json`. OpenAPI describes
transport, authentication, status codes, retry and idempotency semantics; JSON
Schema describes versioned payload shape. A change to either is a protocol
change and requires fixtures and compatibility tests.

- `ai-kodu-runner.dev/v1alpha1` is the legacy Community protocol.
- `ai-kodu-runner.dev/v1beta1` is the next protocol version and introduces typed
  execution requirements in place of client-selected executor names.
- A stable `ai-kodu-runner.dev/v1` is introduced only after the v1beta1 protocol,
  fixtures, control-plane behavior, and at least two executor implementations
  have passed the compatibility suite.

Within a stable API version, changes must be additive and tolerant of unknown
fields. Required-field additions, changed meanings, removed fields, changed
defaults with observable behavior, or changes to result/failure semantics
require a new API version.

`FailureInfo.code` is a stable machine-readable value. The current Community
catalog includes `cancelled`, `timeout`, `command_failed`,
`image_pull_failed`, and `executor_operation_failed`. The latter is the safe
fallback for a Docker/host error that has not yet received a more specific
classification; it is always reported with `kind: infrastructure` and must
not be interpreted as a user command failure. New specific codes are additive;
changing the meaning of an existing code requires a protocol version change.

Every protocol release includes JSON fixtures for parsing an older manifest,
serializing current results and events, and rejecting unsupported requirements
before user code is run.

## Worker protocol

The controller/ingestion protocol used by `runner-worker` is versioned
independently from the user-facing JobSpec API. The worker handshake includes
its protocol version, binary/image digest, supported features, `run_id`, and
`execution_id` before it requests the attempt start permit.
For Kubernetes execution, `execution_id` is the complete Pod UID. It is not
hashed, truncated, or accepted without server-side TokenReview and Pod/owner
verification defined by ADR 0003.

An Enterprise controller supports the worker protocol shipped with its current
minor release and the immediately previous Enterprise minor release (N-1).
Support beyond N-1 requires an explicit release-manifest exception and
qualification evidence. An incompatible worker is rejected before workspace
download or user-code execution.

Within a worker protocol major version, new fields and message kinds must be
additive, bounded, and safely ignored or rejected according to documented
feature negotiation. Changes to event identity, ACK/deduplication semantics,
token scope, terminal-result identity, or canonical state transitions require a
new worker protocol major version.

The worker runs inside an untrusted environment. A compatible version handshake
does not make worker messages authoritative: ingestion and controller validate
identity, scope, size, sequence, lifecycle, verifier policy, object digest, and
Kubernetes evidence as required by ADR 0003.

## Migration and deprecation

When `v1beta1` is released, Community continues to read `v1alpha1` through a
compatibility adapter for at least two subsequent Community minor releases and
at least 90 calendar days, whichever is longer. The adapter maps legacy
`executor: "docker"` only to the `container` requirement.

The Enterprise control plane may continue to parse a legacy request, but it
must route according to server policy and capability matching. A legacy client
can never use `executor: "docker"` to authorize a weaker isolation level.

Deprecation notices appear in release notes, CLI validation output where
applicable, protocol documentation, and the migration guide. Removal occurs
only in a declared minor release before `1.0.0`, or a declared major release
after `1.0.0`.

## Product releases

Community and Enterprise products use release versions independent of the
protocol API. An Enterprise release pins:

- exact Community crate versions and source revision;
- accepted and emitted protocol API versions;
- exact Enterprise component versions;
- runner-worker protocol versions and signed image digest;
- the qualified Kubernetes/containerd/Kata matrix from `SUPPORT.md`;
- image and Helm chart digests.

An Enterprise patch release does not add a new protocol API version or a new
runtime matrix entry. Such additions require an Enterprise minor release and a
fresh qualification run.

## Compatibility gate

A release candidate is blocked if any of the following is missing:

- protocol fixtures for every supported API version;
- Docker and mock-executor conformance results in the public repository;
- Kubernetes-executor compatibility results in the private repository;
- current and N-1 worker-protocol fixtures, forged-message tests, and version
  handshake rejection tests;
- an exact dependency lock/SBOM and a release manifest;
- a documented migration path for any deprecated contract.

## References

- [ADR 0002](decisions/0002-community-docker-enterprise-kubernetes.md)
- [ADR 0003](decisions/0003-enterprise-delivery-and-trust-boundaries.md)
- [ADR 0004](decisions/0004-enterprise-threat-model-and-secure-delivery-gates.md)
- [Security model](SECURITY_MODEL.md)
- [Roadmap](ROADMAP.md)
- [Commercial distribution process](COMMERCIAL_DISTRIBUTION.md)
