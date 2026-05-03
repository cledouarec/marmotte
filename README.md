# Marmotte

Hibernating build cache for Yocto. v1 ships:

- HTTP API compatible with BitBake's `SSTATE_MIRRORS` and `PREMIRRORS`.
- Local content-addressed storage with transparent dedup across projects.
- Project-scoped HTTP Basic auth (read/write roles), admin Bearer API.
- Periodic GC (LRU + TTL + quotas, with pinning).

## Quick start

```bash
cargo build --release
./target/release/marmotte init --config /etc/marmotte/config.toml
./target/release/marmotte serve --config /etc/marmotte/config.toml
```

## v1 non-objectives

- PyPI / NPM / Docker registry backends.
- Web admin UI.
- TUI dashboard.
- Replication / clustering / HA.
- S3 / object-storage backend.
- Native TLS (run behind a reverse proxy).
- Postgres metadata store.
- Range requests / resumable uploads.
- Webhooks.
- Cross-project "smart" GC.
- Named pin sets / snapshots.

These are explicit non-goals for the v1 release. See [docs/operators.md](docs/operators.md) for deployment guidance.
