#!/usr/bin/env bash
# The release's Linux gate: the whole test suite on a Debian or Ubuntu machine, with the reference Postgres 16 (C
# locale, UTC, pg_trgm, port 5416, as tests/PGDIFF.md), the S3 store against a local MinIO, the doctests, the
# installer's test and the performance budgets. Writes linux-gate/summary.md (host, commit, each step, every failing
# test, each marked known or NEW against tests/known_failures.txt) beside each step's log, and exits 1 when a step
# fails or a test fails that isn't a known failure.
#
#   tools/linux-gate.sh            # the gate: on x86_64 Linux, for a release (RELEASING.md step 3)
#   tools/linux-gate.sh --smoke    # a few test binaries only, no budgets: checks the script itself (e.g. in Docker)
#
# Runs as an ordinary user with sudo (a GitHub runner, a VM), or as root (a container: it makes a user `ci` and runs
# the tests as that user, since tests that depend on file permissions behave differently as root). Needs cargo
# (rustup's), curl and apt. Set LINUX_GATE_OUT for another output folder.
set -euo pipefail

smoke=0
phase=setup
for a in "$@"; do
    case "$a" in
    --smoke) smoke=1 ;;
    --run) phase=run ;; # (internal: the tests, as the ordinary user)
    *)
        echo "usage: tools/linux-gate.sh [--smoke]" >&2
        exit 2
        ;;
    esac
done

here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
out=${LINUX_GATE_OUT:-$here/linux-gate}
# a new private folder each run (made in setup, passed on): nothing in it was left by anyone else
work=${LINUX_GATE_WORK:-}
arch=$(uname -m)
case "$arch" in
x86_64) goarch=amd64 ;;
aarch64 | arm64) goarch=arm64 ;;
*)
    echo "unsupported machine: $arch" >&2
    exit 2
    ;;
esac
export LANG=C.UTF-8 LC_ALL=C.UTF-8

if [ "$phase" = setup ]; then
    [ "$(uname -s)" = Linux ] || {
        echo "this is the Linux gate: run it on Linux (or in a Linux container)" >&2
        exit 2
    }
    if [ "$(id -u)" = 0 ]; then sudo=""; else sudo="sudo"; fi
    work=$(mktemp -d /tmp/linux-gate.XXXXXX)
    export LINUX_GATE_WORK=$work
    mkdir -p "$out"
    echo "== packages"
    $sudo apt-get update -qq
    $sudo apt-get install -y -qq curl ca-certificates gnupg build-essential pkg-config python3 >/dev/null
    . /etc/os-release
    if ! [ -x /usr/lib/postgresql/16/bin/initdb ]; then
        curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc | $sudo gpg --dearmor --yes -o /usr/share/keyrings/pgdg.gpg
        echo "deb [signed-by=/usr/share/keyrings/pgdg.gpg] http://apt.postgresql.org/pub/repos/apt ${VERSION_CODENAME}-pgdg main" |
            $sudo tee /etc/apt/sources.list.d/pgdg.list >/dev/null
        $sudo apt-get update -qq
        # (only the binaries: the package's own cluster on 5432 is stopped, the gate makes its own)
        $sudo apt-get install -y -qq postgresql-16 postgresql-client-16 >/dev/null
        $sudo sh -c 'command -v pg_ctlcluster >/dev/null && pg_ctlcluster 16 main stop' 2>/dev/null || true
    fi
    curl -fsSL -o "$work/minio" "https://dl.min.io/server/minio/release/linux-$goarch/minio"
    chmod +x "$work/minio"
    if ! command -v cargo-nextest >/dev/null; then
        mkdir -p "$work/bin"
        target=$([ "$goarch" = amd64 ] && echo x86_64-unknown-linux-gnu || echo aarch64-unknown-linux-gnu)
        curl -fsSL "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-0.9.146/cargo-nextest-0.9.146-$target.tar.gz" |
            tar -xz -C "$work/bin"
    fi
    # the Studio's web UI, built into the binary when Node is there (without it build.rs puts a placeholder page)
    if command -v npm >/dev/null && ! [ -d studio/dist ]; then
        (cd studio && npm ci --silent && npm run build --silent) >"$out/studio.log" 2>&1 || echo "studio: build failed, see studio.log"
    fi
    if [ "$(id -u)" = 0 ]; then
        getent passwd ci >/dev/null || useradd --uid 1000 --create-home --shell /bin/bash ci
        home=$(getent passwd ci | cut -d: -f6)
        # cargo and rustup stay where they are (an image's /usr/local, or root's own ~/.cargo and ~/.rustup), handed to
        # the user, named explicitly (the user's HOME differs) and reachable (a 0700 /root is opened for traversal only)
        cargo_home=${CARGO_HOME:-$(cd "$(dirname "$(command -v cargo)")/.." && pwd)}
        rustup_home=${RUSTUP_HOME:-$(rustup show home 2>/dev/null || echo "$HOME/.rustup")}
        for d in "$cargo_home" "$rustup_home"; do
            [ -d "$d" ] || continue
            chown -R ci: "$d"
            p=$(dirname "$d")
            while [ "$p" != / ]; do chmod a+x "$p"; p=$(dirname "$p"); done
        done
        chown -R ci: "$here" "$out" "$work"
        exec setpriv --reuid=ci --regid="$(id -g ci)" --init-groups env HOME="$home" USER=ci LOGNAME=ci PATH="$PATH" \
            CARGO_HOME="$cargo_home" RUSTUP_HOME="$rustup_home" \
            LINUX_GATE_OUT="$out" LINUX_GATE_WORK="$work" bash "$0" --run $([ $smoke = 1 ] && echo --smoke)
    fi
    exec bash "$0" --run $([ $smoke = 1 ] && echo --smoke)
fi

# ---- the tests, as an ordinary user
export PATH="$work/bin:/usr/lib/postgresql/16/bin:$PATH"
started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
steps=()
status=0

# the reference Postgres 16 (tests/PGDIFF.md): its own folder, socket in /tmp, port 5416
pg=$work/pg16
# trusted over the socket (the oracle tests), a password over TCP (the import and attach tests, Studio's need one)
printf '%s\n' gate-pg >"$work/pgpass"
initdb -D "$pg" --locale=C -E UTF8 -U "$USER" --auth-local=trust --auth-host=scram-sha-256 --pwfile="$work/pgpass" >/dev/null
pg_ctl -D "$pg" -o "-p 5416 -k /tmp -c timezone=UTC -c listen_addresses=127.0.0.1" -l "$work/pg16.log" -w start >/dev/null
for d in template1 postgres; do psql -h /tmp -p 5416 -d "$d" -qc 'create extension if not exists pg_trgm'; done
export PGPORT=5416 CHRONOS_TEST_PG="postgres://$USER:gate-pg@127.0.0.1:5416/postgres"

# MinIO for the S3 store (tests/remote.rs and the review_storage tests), and its bucket
export AWS_ACCESS_KEY_ID=gateminio AWS_SECRET_ACCESS_KEY=gateminio-secret AWS_REGION=us-east-1
export AWS_ENDPOINT=http://127.0.0.1:9000 AWS_ALLOW_HTTP=true
MINIO_ROOT_USER=$AWS_ACCESS_KEY_ID MINIO_ROOT_PASSWORD=$AWS_SECRET_ACCESS_KEY \
    "$work/minio" server "$work/minio-data" --address 127.0.0.1:9000 --quiet >"$work/minio.log" 2>&1 &
minio=$!
cleanup() {
    kill "$minio" 2>/dev/null || true
    pg_ctl -D "$pg" -m fast stop >/dev/null 2>&1 || true
}
trap cleanup EXIT
for _ in $(seq 60); do curl -fs http://127.0.0.1:9000/minio/health/live >/dev/null && break; sleep 1; done
curl -fsS --aws-sigv4 "aws:amz:$AWS_REGION:s3" --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" \
    -X PUT "$AWS_ENDPOINT/chronos-gate" >/dev/null
export CHRONOS_TEST_S3=s3://chronos-gate/run-$$

# one step: its name, its log, the command; recorded pass or fail, never stopping the others. The test run's own
# failures (tests) are judged after, against the known failures; only its other failures (a build error) fail it here.
tests_failed=0
step() {
    local name=$1 log=$2
    shift 2
    echo "== $name"
    local t0=$SECONDS
    if "$@" >"$out/$log" 2>&1; then
        steps+=("| $name | passed | $((SECONDS - t0)) s | \`$log\` |")
    elif [ "$log" = nextest.log ]; then
        steps+=("| $name | tests failed (below) | $((SECONDS - t0)) s | \`$log\` |")
        tests_failed=1
    else
        steps+=("| $name | **failed** | $((SECONDS - t0)) s | \`$log\` |")
        status=1
    fi
    tail -3 "$out/$log"
}

if [ $smoke = 1 ]; then
    step "tests (smoke: crash, remote, attach)" nextest.log \
        cargo nextest run --release --profile ci --features s3 -E 'binary(crash) | binary(remote) | binary(attach)'
else
    step "every test (release, --features s3)" nextest.log \
        cargo nextest run --release --profile ci --workspace --features s3 --no-fail-fast
    step "doctests" doctests.log cargo test --release --doc
    step "installer test" install.log sh tests/install_test.sh
    step "latency budgets (tests/perf.rs)" perf.log cargo test --release --test perf -- --ignored --nocapture
    step "subquery budgets (tests/subquery_perf.rs)" subquery_perf.log \
        cargo test --release --test subquery_perf -- --ignored --nocapture
fi

# failing tests, each known (tests/known_failures.txt) or new
known=$(grep -v '^#' tests/known_failures.txt 2>/dev/null | grep . | tr '\t' ' ' || true)
failed=$(grep -E '^ +(FAIL|SIGABRT|SIGSEGV|SIGKILL|TIMEOUT) \[' "$out/nextest.log" | awk '{print $(NF-1), $NF}' | sort -u || true)
new=""
while IFS= read -r t; do
    [ -n "$t" ] || continue
    grep -qxF "$t" <<<"$known" || new+="$t"$'\n'
done <<<"$failed"
[ -z "$new" ] || status=1
# the run failed but no test did: it didn't build, or nextest itself failed
if [ $tests_failed = 1 ] && [ -z "$failed" ]; then status=1; fi
tally=$(grep -E '^ +Summary \[' "$out/nextest.log" | tail -1 | sed 's/^ *//' || true)

{
    echo "# Linux gate: $([ $status = 0 ] && echo passed || echo FAILED)$([ $smoke = 1 ] && echo ' (smoke run, not the gate)')"
    echo
    echo "- Commit: \`$(git rev-parse HEAD 2>/dev/null || echo unknown)\`$(git describe --tags --always 2>/dev/null | sed 's/^/ (/;s/$/)/')"
    echo "- Host: $(. /etc/os-release && echo "$PRETTY_NAME"), $(uname -m), $(nproc) cores, $(awk '/MemTotal/ {printf "%.0f GB", $2/1048576}' /proc/meminfo)"
    echo "- Toolchain: $(rustc --version), $(cargo nextest --version | head -1)"
    echo "- Reference Postgres: $(postgres --version), C locale, UTC, pg_trgm, port 5416; S3: MinIO at $AWS_ENDPOINT"
    echo "- Started $started, finished $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo
    echo "| Step | Result | Time | Log |"
    echo "|---|---|---|---|"
    printf '%s\n' "${steps[@]}"
    echo
    echo "Tests: ${tally:-no summary line (see nextest.log)}"
    echo
    echo "## New failures (not in tests/known_failures.txt)"
    echo
    if [ -n "$new" ]; then printf '%s' "$new" | sed 's/^/- `/;s/$/`/'; else echo "None."; fi
    echo
    echo "## Known failures that failed"
    echo
    k=$(grep -xF -f <(printf '%s\n' "$known") <<<"$failed" || true)
    if [ -n "$k" ]; then sed 's/^/- `/;s/$/`/' <<<"$k"; else echo "None."; fi
} >"$out/summary.md"
cat "$out/summary.md"
exit $status
