# Operators guide

## Reverse proxy (TLS)

Marmotte serves plain HTTP. Terminate TLS upstream (nginx/Caddy/Traefik). Forward
the original `Authorization` header.

## Sizing

- 100 GB storage ≈ 1k Yocto sstate artifacts. Plan for 2–5× this for a busy team.
- SQLite handles tens of millions of rows easily; the bottleneck is disk.

## Backups

Stop the service or use `sqlite3 .backup`, then snapshot `<storage_root>/blobs/`.
The two halves can drift by seconds in a hot copy; reconcile with
`POST /api/v1/admin/gc/orphan-scan?dry_run=false`.

## Routine ops

- Periodic GC runs every `gc.interval_secs` (default 300s).
- Manually trigger via `POST /api/v1/admin/gc/run`.
- Inspect orphans with `gc/orphan-scan?dry_run=true` first.

## Pinning a release

```bash
# List build artifacts and pin them.
curl -s -H "Authorization: Bearer $MAR_ADMIN" \
    "$BASE/api/v1/admin/projects/$PID/entries?path_prefix=release-1.2/" \
    | jq '.entries[].id' | while read eid; do
        curl -X POST -H "Authorization: Bearer $MAR_ADMIN" \
            "$BASE/api/v1/admin/projects/$PID/entries/$eid/pin"
    done
```
