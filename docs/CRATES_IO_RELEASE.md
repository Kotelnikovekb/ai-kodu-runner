# crates.io publication checklist

This repository is prepared for a coordinated Community release. Publishing is
not performed locally by default; the protected GitHub release workflow uses
the `CARGO_REGISTRY_TOKEN` secret.

## Before the first release

- Confirm the package names: `ai-kodu-runner-protocol`, `ai-kodu-runner-core`,
  `ai-kodu-runner-executor-docker`, and `ai-kodu-runner`.
- The root binary package intentionally excludes `examples/`, `docs/`,
  `.github/`, tool images, local configuration and generated artifacts. Those
  remain repository materials, not runtime crate contents.
- Reserve/check all four package names before the first upload.
- Create a crates.io API token restricted to the required packages if the
  account supports package-scoped tokens.
- Add the token as a masked GitHub Actions secret named
  `CARGO_REGISTRY_TOKEN`.
- Protect version tags and require review before creating a `v*` tag.
- Confirm `LICENSE.md`, `NOTICE`, README files and repository URLs are present
  in every package archive.

## Local verification

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo package -p ai-kodu-runner-protocol --allow-dirty
cargo package -p ai-kodu-runner-core --allow-dirty
cargo package -p ai-kodu-runner-executor-docker --allow-dirty
cargo package -p ai-kodu-runner --allow-dirty
```

Inspect the archive before publishing:

```sh
tar -tzf target/package/ai-kodu-runner-0.1.0.crate
```

It should contain the binary source and package README, but not examples,
local databases, runner configuration, or internal audit documents.

Use `cargo publish -p <crate> --dry-run --locked` for a registry validation
without uploading. The dependency order is `ai-kodu-runner-protocol`,
`ai-kodu-runner-core`, `ai-kodu-runner-executor-docker`, then `ai-kodu-runner`.

## Release procedure

1. Update all public package versions together while the workspace is below
   `1.0.0`.
2. Regenerate and review `Cargo.lock`.
3. Run the local verification commands.
4. Create and push an annotated `v<version>` tag.
5. GitHub Actions publishes crates in dependency order and creates binary
   release assets.
6. Verify each crate page, checksum, README and dependency resolution.
7. Record the release in the Enterprise release manifest before consuming it
   from the private GitLab repository.

The workflow intentionally publishes only on version tags. A pull request or a
normal branch push never publishes to crates.io.
