# Marmotte

A hibernating build cache for [Yocto](https://www.yoctoproject.org/) / OpenEmbedded.

## The problem

A from-scratch Yocto build is brutally expensive: thousands of recipes compiled
from source, gigabytes of upstream tarballs fetched over the network, tens of
minutes (often hours) of CPU burned every time a fresh worker, a new branch, or
a wiped CI runner starts a build.

BitBake already knows how to avoid most of that work — it just needs somewhere
to look:
- **`PREMIRRORS`** — a remote download mirror, so source tarballs and patches
  are fetched once and reused forever (and survive upstream going away).
- **`SSTATE_MIRRORS`** — a remote shared-state cache, so a recipe whose inputs
  haven't changed is *restored* instead of *rebuilt*.

Marmotte is that "somewhere". It's a small, self-hosted HTTP server that speaks
exactly the protocol BitBake expects for those two mechanisms, backed by a
content-addressed blob store with transparent cross-project deduplication and
automatic garbage collection so it doesn't grow without bound.

Think of it as a warm burrow your builds crawl into: most of the work is already
done, they just have to wake up.

## Key features

- **Drop-in BitBake integration** — HTTP `GET`/`HEAD`/`PUT` under
  `/sstate/<project>/...` and `/downloads/<project>/...`, ready to paste into
  `SSTATE_MIRRORS` and `PREMIRRORS`.
- **Content-addressed storage with dedup** — blobs are keyed by SHA-256, so the
  same artifact shared by ten projects is stored once. Sharded on-disk layout
  (`blobs/aa/bb/<hash>`) keeps directories small.
- **Project isolation + role-based auth** — each project gets its own HTTP Basic
  credentials with a `read` or `write` role. A separate Bearer-token admin API
  manages everything.
- **Automatic garbage collection** — periodic sweeps combine TTL expiry
  (per-kind defaults, overridable per project), per-project quotas, a global
  storage quota with LRU eviction, and **pinning** to protect release artifacts.
- **Operable** — `/healthz` and `/readyz` probes, Prometheus `/metrics`,
  structured JSON logging, graceful shutdown, an audit log of admin actions, and
  an orphan-scan command to reconcile disk and DB after a hot backup.
- **A `push` client** — bulk-upload an existing `sstate-cache/` or `downloads/`
  directory to seed the cache, with concurrency, resumability (skips what the
  server already has), and `--dry-run`.

## How it fits together

```mermaid
flowchart TD
    builders["Yocto builders / CI<br/>SSTATE_MIRRORS + PREMIRRORS"]
    admin["Admin / operator"]

    marmotte["<b>Marmotte</b><br/>/sstate · /downloads · admin API<br/>+ periodic GC"]

    blobs[("blob store<br/>(content-addressed)")]
    db[("SQLite metadata")]

    builders -- "HTTP Basic" --> marmotte
    admin -- "Bearer token" --> marmotte
    marmotte --> blobs
    marmotte --> db
```

## Quick start

### 1. Build

```bash
cargo build --release
```

### 2. Write a config file

Create `/etc/marmotte/config.toml` (see [Configuration](#configuration) for
every option):

```toml
[server]
listen        = "0.0.0.0:9090"
storage_root  = "/var/lib/marmotte"

[database]
path = "/var/lib/marmotte/marmotte.db"

[gc]
default_ttl_sstate_days    = 30      # un-touched sstate entries expire after 30 days
default_ttl_downloads_days = 365     # downloads are cheap to keep, expensive to refetch
global_quota_bytes         = 214748364800   # 200 GiB hard cap across all projects

[logging]
level  = "info"
format = "pretty"     # use "json" in production
```

### 3. Initialize storage and mint the first admin token

```bash
./target/release/marmotte init \
    --config /etc/marmotte/config.toml \
    --admin-token-out /etc/marmotte/admin.token
# creates <storage_root>/blobs and <storage_root>/tmp,
# runs DB migrations, and writes the admin token secret (mode 0600).
# Without --admin-token-out the secret is printed to stdout (once).
```

### 4. Run the server

```bash
./target/release/marmotte serve --config /etc/marmotte/config.toml
```

Marmotte serves plain HTTP — put it behind nginx/Caddy/Traefik for TLS and make
sure the proxy forwards the original `Authorization` header. See
[`docs/operators.md`](docs/operators.md).

### 5. Create a project and its credentials

All admin calls use the Bearer token from step 3.

```bash
BASE=http://localhost:9090
ADMIN=$(cat /etc/marmotte/admin.token)

# Create a project (quota_bytes / ttl_seconds are optional; null = use global defaults)
curl -s -X POST "$BASE/api/v1/admin/projects" \
    -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    -d '{"name":"acme-bsp","quota_bytes":null,"ttl_seconds":null}'
# → {"id": 1, "name": "acme-bsp", ...}

# Mint a write key for build hosts and a read key for everyone else
curl -s -X POST "$BASE/api/v1/admin/projects/1/keys" \
    -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    -d '{"role":"write","label":"ci-uploader"}'
# → {"id": 1, "role": "write", "secret": "Xy3...43chars...", ...}   ← shown ONCE

curl -s -X POST "$BASE/api/v1/admin/projects/1/keys" \
    -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    -d '{"role":"read","label":"developers"}'
```

The `secret` is returned only at creation time — store it somewhere safe.

### 6. Point Yocto at it

In your distro/machine/local `conf` (substitute project name and key):

```python
# Read-and-write sstate mirror. CI builders use the write key so successful
# builds populate the cache; developers can use a read-only key here instead.
SSTATE_MIRRORS ?= "file://.* http://acme-bsp:WRITE_KEY@cache.internal:9090/sstate/acme-bsp/PATH \n"

# Source-download mirror. PATH is replaced by BitBake with the artifact name.
PREMIRRORS ?= "\
    git://.*/.*    http://acme-bsp:READ_KEY@cache.internal:9090/downloads/acme-bsp/ \n \
    ftp://.*/.*    http://acme-bsp:READ_KEY@cache.internal:9090/downloads/acme-bsp/ \n \
    http://.*/.*   http://acme-bsp:READ_KEY@cache.internal:9090/downloads/acme-bsp/ \n \
    https://.*/.*  http://acme-bsp:READ_KEY@cache.internal:9090/downloads/acme-bsp/ \n"
```

The credentials are standard HTTP Basic: the **username is the project name**
and the **password is the API key**. The project segment in the URL must match
the credential's project, or the request is rejected with `403`.

### 7. (Optional) Seed the cache from an existing build

If you already have a populated `sstate-cache/` or `downloads/` directory, push
it in bulk instead of waiting for builds to fill the cache:

```bash
./target/release/marmotte push \
    --project  acme-bsp \
    --kind     sstate \
    --base-url http://cache.internal:9090/ \
    --api-key  WRITE_KEY \
    --concurrency 16 \
    /path/to/build/sstate-cache
# HEADs each file first and skips what the server already has;
# retries 5xx with exponential backoff; prints  total N | pushed X | skipped Y | failed Z
# add --dry-run to see what it would do without uploading
```

## Configuration

Config is a TOML file (path via `--config`, default `/etc/marmotte/config.toml`).
Any value can be overridden by an environment variable named `MARMOTTE_<SECTION>__<KEY>`
(double underscore between levels), e.g. `MARMOTTE_SERVER__LISTEN=0.0.0.0:7777`.

### `[server]`

| Key                   | Type     | Default        | Meaning |
|-----------------------|----------|----------------|---------|
| `listen`              | `host:port` | *(required)* | Address the HTTP server binds to. |
| `storage_root`        | path     | *(required)*   | Root directory of the blob store. Marmotte creates `blobs/` and `tmp/` underneath. |
| `request_timeout_secs`| integer  | `300`          | Max seconds per HTTP request — keep it generous, large sstate uploads on slow links can take a while. |
| `upload_max_bytes`    | integer  | `5368709120` (5 GiB) | Maximum accepted upload body size. |

### `[database]`

| Key              | Type    | Default | Meaning |
|------------------|---------|---------|---------|
| `path`           | path    | *(required)* | SQLite database file. Migrations run automatically. Put it on the same fast disk as `storage_root`. |
| `busy_timeout_ms`| integer | `5000`  | How long to wait on a locked DB before erroring. |

### `[gc]`

| Key                      | Type    | Default | Meaning |
|--------------------------|---------|---------|---------|
| `interval_secs`          | integer | `300`   | Seconds between scheduled GC sweeps. |
| `default_ttl_sstate_days`| integer | *(required)* | Default time-to-live for `sstate` entries, counted from last access. Overridable per project via `ttl_seconds`. |
| `default_ttl_downloads_days` | integer | *(required)* | Same, for `downloads` entries. |
| `global_quota_bytes`     | integer | *(required)* | Hard cap on total blob storage. When exceeded, the global LRU phase evicts the least-recently-accessed unpinned entries until it fits. |
| `trigger_threshold_pct`  | integer 1–100 | `90` | When usage rises above this percentage of `global_quota_bytes`, a `PUT` also kicks off a background GC sweep (the response is not blocked). |

### `[auth]` (optional)

| Key                    | Type    | Default | Meaning |
|------------------------|---------|---------|---------|
| `verify_cache_size`    | integer | `1024`  | Max entries in the credential-verification cache. Verifying a credential runs Argon2 (deliberately slow); this cache amortizes it across repeated requests from the same client. |
| `verify_cache_ttl_secs`| integer | `300`   | How long a cached verification result stays valid. |

### `[logging]` (optional)

| Key      | Type   | Default  | Meaning |
|----------|--------|----------|---------|
| `level`  | string | `"info"` | Minimum log level; accepts `RUST_LOG`-style filters (`"debug"`, `"warn"`, `"marmotte=debug,info"`, …). |
| `format` | string | `"json"` | `"json"` for structured logs (production / log pipelines) or `"pretty"` for human-readable local development. |

A minimal valid config (used by the test suite) lives at
[`crates/marmotte-core/tests/fixtures/config_minimal.toml`](crates/marmotte-core/tests/fixtures/config_minimal.toml).

## HTTP API

### Yocto cache (project HTTP Basic auth — username = project, password = API key)

| Method | Path | Role | Notes |
|--------|------|------|-------|
| `GET` / `HEAD` | `/sstate/{project}/{*path}` | `read` or `write` | Returns the blob with `ETag`, `Content-Length`, `Cache-Control: public, immutable, max-age=31536000`. `404` on miss. |
| `PUT` | `/sstate/{project}/{*path}` | `write` | Stores the body, dedups by SHA-256. `201 Created` with `ETag`. |
| `GET` / `HEAD` | `/downloads/{project}/{filename}` | `read` or `write` | As above; downloads use a single filename segment. |
| `PUT` | `/downloads/{project}/{filename}` | `write` | As above. |

### Public (no auth)

| Method | Path | Notes |
|--------|------|-------|
| `GET` | `/healthz` | Liveness — always `200 ok` if the process is up. |
| `GET` | `/readyz` | Readiness — `200` if the DB is reachable and storage is writable, otherwise `503` with a reason. |
| `GET` | `/metrics` | Prometheus text exposition (cache hits/misses per project & kind, GC stats, …). |

### Admin API — `Authorization: Bearer <admin-token>`

All paths are under `/api/v1/admin`.

**Projects** — `quota_bytes` and `ttl_seconds` are nullable; `null` means "use the global defaults". `PATCH` distinguishes "absent" (leave unchanged) from explicit `null` (clear).

- `POST /projects` `{ "name", "quota_bytes": null|int, "ttl_seconds": null|int }` → `201`
- `GET /projects` · `GET /projects/{id}` · `PATCH /projects/{id}` · `DELETE /projects/{id}`

**API keys** (per project — these are the HTTP Basic passwords your builds use)

- `POST /projects/{id}/keys` `{ "role": "read"|"write", "label": null|string }` → `201` with `secret` (returned only here)
- `GET /projects/{id}/keys` (no secrets) · `DELETE /projects/{id}/keys/{kid}` (revoke)

**Admin tokens** (Bearer tokens for this API)

- `POST /tokens` `{ "label": null|string }` → `201` with `secret` (returned only here)
- `GET /tokens` · `DELETE /tokens/{id}` (revoke)

**Cache entries**

- `GET /projects/{id}/entries?kind=sstate|downloads&pinned=true|false&path_prefix=…&sort=path|last_accessed|size|created_at&order=asc|desc&limit=100&cursor=…&count=true` → paginated list
- `GET /projects/{id}/entries/{eid}` · `DELETE /projects/{id}/entries/{eid}`
- `POST /projects/{id}/entries/{eid}/pin` / `DELETE …/pin` — protect an entry from GC (e.g. a release artifact) / release the protection

**Garbage collection** (`?dry_run=true` reports without changing anything)

- `POST /gc/run` → `{ evicted_entries, evicted_bytes, freed_blobs, usage_global_bytes, dry_run }`
- `POST /gc/orphan-scan` → `{ disk_orphans_removed, db_orphans_removed, dry_run }` — reconcile on-disk blobs with DB rows (run after a hot backup)

**Stats & audit**

- `GET /stats` → `{ global_bytes, projects }`
- `GET /stats/projects/{id}` → project + usage + hit/put counters
- `GET /audit?since=<ts>&limit=100` → recent admin actions

## How garbage collection works

Every `interval_secs` (and on demand via `POST /gc/run`, and softly after a
`PUT` once usage passes `trigger_threshold_pct`), Marmotte runs, in order:

1. **TTL expiry** — for each kind, evict unpinned entries not accessed within
   their effective TTL (`project.ttl_seconds` if set, else the per-kind default).
2. **Per-project quota** — for each project with `quota_bytes`, evict its
   unpinned entries least-recently-accessed-first until it's under quota.
3. **Global LRU** — if total blob storage exceeds `global_quota_bytes`, evict
   unpinned entries across all projects, oldest-access-first, until it fits.
4. **Blob cleanup** — delete blob files and rows whose reference count dropped to
   zero.

**Pinned** entries are never evicted by steps 1–3. Pin release builds you need
to keep; everything else ages out naturally.

`POST /gc/orphan-scan` is separate: it removes disk files with no DB row and DB
rows whose file is missing — useful after an unclean shutdown or a hot backup
where the SQLite file and `blobs/` drifted by a few seconds.

## Storage layout

```
<storage_root>/
├── blobs/
│   └── aa/bb/aabbccdd…   ← file content, named by its SHA-256 (sharded by the
│                            first two bytes of the hash → ≤ 65 536 directories)
└── tmp/
    └── <uuid>            ← in-flight uploads; written here, SHA-256'd on the
                             fly, then atomically renamed into blobs/ (if a blob
                             with that hash already exists, the temp file is
                             discarded — that's the dedup)
```

Blobs are immutable: a given hash always maps to the same bytes, which is why
responses are served with `Cache-Control: immutable` and clients/CDNs can cache
them forever. SQLite (at `database.path`) holds projects, credentials, the
entry → blob mapping with reference counts, the audit log, and stats.

## Repository layout

| Crate | Purpose |
|-------|---------|
| `crates/marmotte-core`   | Storage, DB layer + migrations, auth, GC, config. |
| `crates/marmotte-server` | Axum HTTP server: Yocto routes, admin API, middleware, metrics, observability. |
| `crates/marmotte-cli`    | The `marmotte` binary: `init`, `serve`, `push`. |
| `tests/integration`      | End-to-end tests (round-trip, auth, GC, pagination, concurrent PUT). |

See [`docs/operators.md`](docs/operators.md) for reverse-proxy setup, sizing,
backups, and routine operations, and [`CONTRIBUTING.md`](CONTRIBUTING.md) to
hack on it.

## License

Apache-2.0.
