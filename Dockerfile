# A container running `chronos serve`: Postgres protocol on 5432, HTTP API on 7070, the database
# in the volume /data, as an unprivileged user.
#
#   docker build -t chronos .
#   export CHRONOS_TOKEN=$(openssl rand -hex 24) CHRONOS_ADMIN_TOKEN=$(openssl rand -hex 24)
#
# The default command serves TLS and needs a certificate and key, mounted and named in the environment
# (a container started without them exits with "refusing to listen on 0.0.0.0:5432 without TLS"):
#   docker run -d --name chronos -e CHRONOS_TOKEN -e CHRONOS_ADMIN_TOKEN \
#     -v "$PWD/certs:/certs:ro" -e CHRONOS_TLS_CERT=/certs/cert.pem -e CHRONOS_TLS_KEY=/certs/key.pem \
#     -p 5432:5432 -p 7070:7070 -v chronos-data:/data chronos
#   psql "postgres://app:$CHRONOS_ADMIN_TOKEN@db.example.com:5432/main?sslmode=verify-full"  # the database's own user
#   psql "postgres://bot:$CHRONOS_TOKEN@db.example.com:5432/main?sslmode=verify-full"        # the guest
#
# Without TLS, only when nothing but the host itself can reach the ports: say so with --insecure-no-tls
# and publish them on the host's loopback only (it warns at start):
#   docker run -d --name chronos -e CHRONOS_TOKEN -e CHRONOS_ADMIN_TOKEN \
#     -p 127.0.0.1:5432:5432 -p 127.0.0.1:7070:7070 -v chronos-data:/data \
#     chronos serve /data --pg 0.0.0.0:5432 --listen 0.0.0.0:7070 --insecure-no-tls
#
# CHRONOS_TOKEN is required: the server listens on 0.0.0.0 inside the container, and `chronos serve`
# refuses to start on an address that isn't loopback without one, or with one shorter than 24
# characters. It is the Postgres password (any user name) and the HTTP bearer token that lets a
# client in as the guest; CHRONOS_ADMIN_TOKEN is the database's own user, for you and the application
# that owns the data (keep it away from agents). Pass both in the environment (`-e`, which takes them
# from yours), never on a command line. Stop with `docker stop` (SIGTERM closes the database cleanly).
#
# Base images are pinned by version. TODO(owner): pin them by digest too (docker buildx imagetools
# inspect <image>) before publishing an image, as Cross.toml pins the release build's images.

# --- the Studio's web UI, embedded in the binary by build.rs (without it a placeholder page stands in)
FROM node:22.20.0-bookworm-slim AS studio
WORKDIR /src/studio
COPY studio/package.json studio/package-lock.json ./
RUN npm ci
COPY studio/ ./
RUN npm run build

# --- the binary
FROM rust:1.95.0-slim-bookworm AS build
# --build-arg JOBS=8 to build on fewer cores than the machine has
ARG JOBS
WORKDIR /src
COPY . .
COPY --from=studio /src/studio/dist studio/dist
RUN CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --release --locked --features mongo ${JOBS:+-j "$JOBS"}

# --- what runs: Debian slim (glibc), tzdata for named time zones, CA certificates for
# `chronos import` over TLS with sslmode=verify-full
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --home-dir /data --no-create-home --shell /usr/sbin/nologin chronos \
    && mkdir /data && chown chronos:chronos /data && chmod 0700 /data
COPY --from=build /src/target/release/chronos /usr/local/bin/chronos
USER chronos:chronos
VOLUME ["/data"]
EXPOSE 5432 7070
ENTRYPOINT ["chronos"]
# TLS from CHRONOS_TLS_CERT and CHRONOS_TLS_KEY; without them serve refuses to start (see the top of this file)
CMD ["serve", "/data", "--pg", "0.0.0.0:5432", "--listen", "0.0.0.0:7070"]
