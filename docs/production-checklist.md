# Production checklist

Every limit below is **off** when a database is made. That keeps a laptop, a test and an embedded database working
without setup, and nothing is discarded or refused that you didn't ask for. A server that agents share needs most of
them on. `chronos serve` says which are still off as it starts:

```
chronos: warning: limits off: statement_timeout, max_worlds, world_idle_ttl, CHRONOS_MAX_DISK_BYTES, CHRONOS_MAX_MEMORY_BYTES. See docs/production-checklist.md ...
```

The settings (`ALTER SYSTEM SET`) are kept in the database and need the database's own user (the admin token, or a
superuser role). The `CHRONOS_` variables are read where the process starts, and nothing inside the database can
raise them.

## Limits

| Limit | What it protects | A starting value | Set it | Check it |
|---|---|---|---|---|
| `statement_timeout` | a runaway query holding a thread and memory | `'30s'` | `ALTER SYSTEM SET statement_timeout = '30s'` (each session starts with it; one may change its own) | `SHOW statement_timeout` |
| per agent: `max_query_ms`, `max_concurrent`, `max_memory_mb` | one agent taking the whole server | `max_query_ms = 30000`, `max_concurrent = 4`, `max_memory_mb = 512` | `CREATE AGENT ... WITH (...)` / `ALTER AGENT ...` | `SHOW AGENTS` |
| `max_worlds` | forks piling up without end (each pins the pages it was forked from) | a few times the worlds you expect live at once, e.g. `1000` | `ALTER SYSTEM SET max_worlds = 1000`: forks past it are refused (`53400`) | `SHOW max_worlds`, `SHOW WORLDS` |
| `world_idle_ttl` | worlds nobody merges or drops, kept forever | `'7 days'` | `ALTER SYSTEM SET world_idle_ttl = '7 days'`: the background worker discards worlds unused that long (their unmerged work goes with them), logged and audited as `system` | `SHOW world_idle_ttl` |
| `CHRONOS_MAX_DISK_BYTES` | the disk filling up | what the volume can spare, less a margin | in the server's environment: writes past it are refused (`53100`, HTTP `507`); reads, deletes, drops and merges go on | `SHOW DISK` (`total`, `limit`) |
| `CHRONOS_MAX_MEMORY_BYTES` | the process growing until the host kills it | about three quarters of the memory the process may use | in the server's environment: new statements are refused (`53200`) while it holds more | the warning `chronos serve` prints as it starts |
| `CHRONOS_LIMITS` | a database's admin raising the limits above, on a host that runs databases for others | a file of ceilings (`max_worlds`, `statement_timeout_ms`, `world_idle_ttl_ms`, `history_retention_ms`) | in the server's environment | `ALTER SYSTEM SET` past a ceiling is refused |
| `history_retention` | history growing without end (on by default: `1 hour`) | as far back as you need to time travel or undo | `ALTER SYSTEM SET history_retention = '7 days'` | `SHOW history_retention` |

None of these has a default that is on, on purpose: a disk or memory cap has no right value without the host's size,
and discarding idle worlds by default would lose work nobody merged yet. See [operations](operations.md#limits-and-cleanup)
for each in detail.

## Access

- **Tokens.** `chronos serve --token T --admin-token A` (or `CHRONOS_TOKEN`, `CHRONOS_ADMIN_TOKEN`): `T` lets a client in
  as the guest, `A` is the database's own user. Make them different (`openssl rand -hex 24` each). Give agents their
  own tokens (`CREATE AGENT`), never `A`. See [security](security.md).
- **`protect_main`.** `ALTER SYSTEM SET protect_main = on`: `main`'s rows change only by merging a world, for every user.
  A database `chronos` makes starts with it on; check an older or imported one with `SHOW protect_main`.
- **Merge policies.** Hold the merges a person should see (`CREATE MERGE POLICY ... WITH (max_deletes = 0, ...)`) and give
  each agent one. See [merge policies](guides/merge-policies.md).

## Transport and storage

- **TLS.** `--tls-cert` and `--tls-key` (both, or neither). A server on an address other than loopback refuses to start
  without a token, and without TLS unless given `--insecure-no-tls`. See [operations](operations.md#tls).
- **Encryption at rest.** `CHRONOS_KEY` (from `chronos keygen`) when the folder is made. See
  [operations](operations.md#encryption-at-rest).
- **Rollback detection.** `--anchor <file>` outside the folder, somewhere whoever can write the folder can't. See
  [operations](operations.md#rollback-detection-anchor).

## Backups

- `chronos <folder> backup <new folder>` copies the database while it runs; `chronos restore <backup> <new folder>`
  starts one from it and verifies it. Keep backups where an agent's credentials can't reach them, and try a restore
  before you need one. See [operations](operations.md#backups) and [what Chronos protects](what-chronos-protects.md).

## Watching it

- `/metrics` (Prometheus). See [operations](operations.md#metrics).
- `SHOW SECURITY AUDIT`, `SHOW AUDIT`: who did what.
