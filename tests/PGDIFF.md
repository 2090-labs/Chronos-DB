# The reference Postgres

Some tests run the same SQL on Chronos and on a real Postgres and compare the answers: `pgdiff`, `pgcompat`,
`schemas`, `copy`, `read_committed`, `pgcrypto`, `scram`, `pg_dump`. They find that Postgres through its socket in
`/tmp`, on the port `PGPORT` names (5432 when it's unset). Without one they skip their Postgres half, unless
`CHRONOS_REQUIRE_ORACLE` is set, and then they fail.

The answers they expect are those of **Postgres 16 with the C collation, the UTC time zone and `pg_trgm`**. A
Postgres made another way gives other answers in places (the sort order of text under a locale's collation, a
newer version's error codes and catalog columns, a missing extension), and those show up as failures that aren't
Chronos's.

## Start one

A private server on port 5416, next to whatever runs on 5432 (Homebrew's `postgresql@16`):

```bash
B=/opt/homebrew/opt/postgresql@16/bin
P=~/.cache/chronos-pg16
export LC_ALL=C   # (without a valid locale, macOS stops the server as it starts)
$B/initdb -D $P --locale=C -E UTF8 -U $USER
$B/pg_ctl -D $P -o "-p 5416 -k /tmp -c timezone=UTC" -l $P.log start
for d in template1 postgres; do $B/psql -h /tmp -p 5416 -d $d -c 'create extension if not exists pg_trgm'; done
```

Then point the tests at it:

```bash
PGPORT=5416 ctest exec -- cargo test --release --test all pgdiff::
```

`ctest` sets `PGPORT=5416` itself for an engine run when that socket (`/tmp/.s.PGSQL.5416`) exists and `PGPORT` is
unset. Stop it with `$B/pg_ctl -D $P stop`.

## What still differs

The tests that fail against this server even so are listed in `tests/known_failures.txt`, each with the reason.
