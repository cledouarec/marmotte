# Contributing to Marmotte

Contributions are welcome! Please feel free to submit a Pull Request.

## Table of Contents

<details>
<summary>Expand contents</summary>

- [Development](#development)
  - [Prerequisites](#prerequisites)
  - [Building](#building)
  - [Project Structure](#project-structure)
- [How to Contribute](#how-to-contribute)
  - [Commit Convention](#commit-convention)
- [Code of Conduct](#code-of-conduct)

</details>

## Development

### Prerequisites

- Rust 1.94+ (2024 edition) — see [Cargo.toml](Cargo.toml) `workspace.package`
- SQLite (used by `sqlx` for the metadata store)

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run all tests (unit + integration)
cargo test --workspace

# Run lints (clippy is configured with pedantic warnings)
cargo clippy --workspace --all-targets -- -D warnings

# Format
cargo fmt --all
```

Run a local server against a temporary config:

```bash
cargo run -p marmotte-cli -- init --config /tmp/marmotte.toml
cargo run -p marmotte-cli -- serve --config /tmp/marmotte.toml
```

#### Docker

A multi-stage [Dockerfile](Dockerfile) builds a minimal distroless image (non-root, port 8080, data volume at `/var/lib/marmotte`):

```bash
# Build the image
docker build -t marmotte:dev .

# Initialize a config on the host, then run the server
mkdir -p ./data ./etc
docker run --rm -v "$PWD/etc:/etc/marmotte" marmotte:dev \
    init --config /etc/marmotte/config.toml

docker run --rm -p 8080:8080 \
    -v "$PWD/etc:/etc/marmotte" \
    -v "$PWD/data:/var/lib/marmotte" \
    marmotte:dev
```

The default `CMD` is `serve --config /etc/marmotte/config.toml`; override `ENTRYPOINT`/`CMD` for one-off CLI invocations.

### Project Structure

This is a Cargo workspace with three crates plus integration tests:

```
crates/
├── marmotte-core/      # Library: storage, DB, auth, GC, models, config
│   └── src/
│       ├── db/             # SQLite access (projects, api_keys, blobs, entries, stats, admin_tokens)
│       ├── auth.rs         # Argon2-based credential verification
│       ├── storage.rs      # Content-addressed blob store
│       ├── gc.rs           # LRU + TTL + quota garbage collection
│       ├── models.rs       # Domain types
│       ├── config.rs       # TOML/env configuration (figment)
│       └── error.rs
│
├── marmotte-server/    # Library: axum HTTP server, routes, middleware
│   └── src/
│       ├── routes/         # public, yocto, admin endpoints
│       ├── middleware/     # auth_project (Basic), auth_admin (Bearer)
│       ├── observability.rs
│       ├── metrics.rs      # Prometheus exposition
│       └── state.rs
│
└── marmotte-cli/       # Binary: `marmotte` CLI (init, serve, push)
    └── src/
        ├── commands/       # init, serve, push subcommands
        ├── cli.rs
        └── main.rs

tests/
└── integration/        # End-to-end HTTP tests against the server
```

Operator-facing documentation lives in [docs/operators.md](docs/operators.md).

## How to Contribute

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Make sure `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass
4. Commit your changes using [Conventional Commits](https://www.conventionalcommits.org/) format
5. Push to the branch (`git push origin feature/amazing-feature`)
6. Open a Pull Request

Releases are automated through [release-plz](.release-plz.toml) on merge to `main`.

### Commit Convention

We use [Conventional Commits](https://www.conventionalcommits.org/) format:

| Prefix | Description |
|--------|-------------|
| `feat:` | New features |
| `fix:` | Bug fixes |
| `docs:` | Documentation changes |
| `refactor:` | Code refactoring |
| `test:` | Test additions |
| `build:` | Build system and dependencies |
| `style:` | Code style and formatting |
| `ci:` | CI/CD configuration changes |

## Code of Conduct

Please read our [Code of Conduct](CODE_OF_CONDUCT.md) before participating in this project.
