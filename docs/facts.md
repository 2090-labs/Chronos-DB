---
title: Headline facts
---

The numbers other pages quote, in one place. When one changes, change it here and in the pages that repeat it. Each says where it comes from, so it can be checked.

| Fact | Value | Source |
|---|---|---|
| Current release | **0.6.2** (beta), 2026-11-19 | `version` in `Cargo.toml`; the `## 0.6.2` section of `CHANGELOG.md` |
| Postgres's regression suite, by line | **88.98%** of the expected lines; **66 of 219** files exactly the same. Measured 2026-10-27 (#274), on Postgres 16's regression files | `tests/regress_floor.tsv` (the floor that run set); [how it's counted](postgres-compatibility.md#where-it-stands-beta) |
| Postgres's regression suite, by statement | **85.41%** of 44,942 statements give exactly Postgres's output (87.29% counting partial matches). Measured on the earlier run of 2026-10-22, when the line figure was 86.24% | [Postgres compatibility](postgres-compatibility.md#where-it-stands-beta) |
| Differential tests | **495** tests in `tests/pgdiff.rs`, each statement giving the same rows or the same SQLSTATE class on Chronos and on a reference Postgres | count of `#[test]` in `tests/pgdiff.rs` |
| Known test failures | **62** engine tests that fail against the Mac's local Postgres 17 | lines of `tests/known_failures.txt` (comments not counted) |
| Postgres version tested against | **17** as the reference server: the differential and oracle tests, `CHRONOS_TEST_PG`, CI's `test` step, and the Mac's local Postgres. **16** for what Chronos aims to match: the wire protocol, the SQL, the settings and catalog, and the regression files (16.14's in `cloudbuild/regress.yaml` and `tests/regress_expected.sh`) | `tests/pgdiff.rs` `oracle`, `cloudbuild/ci.yaml`, `cloudbuild/regress.yaml` |

The regression figures are as old as their dates: no full regression run has been recorded since. `cloudbuild/regress.yaml` would run it every night, but GCP Cloud Build has been off since 2026-11-03, so nothing runs it today.
