# One image: the Rust server, the exported dashboard, tmux, git and a pinned Reflex.
# The agent CLIs (claude, codex, agy) and their sign-ins stay on the host and are mounted at the same paths
# (see docker-compose.yml), so the container and the host share one login per account.

# ---- dashboard: Next.js static export
FROM oven/bun:1.4.1 AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend/ ./
RUN bun run build

# ---- server
FROM rust:1.98-slim-trixie AS backend
WORKDIR /src/backend
COPY backend/ ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/backend/target \
    cargo build --release --locked && cp target/release/map /usr/local/bin/map

# ---- runtime: same Ubuntu as the host, so the mounted agent binaries find the libraries they were built for
FROM ubuntu:26.04

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git less procps tini tmux \
    && rm -rf /var/lib/apt/lists/*

# Reflex (https://reflex.gist.rs), pinned and checked against the release's SHA256SUMS.
ARG TARGETARCH=amd64
ARG REFLEX_VERSION=v0.2.4
ARG REFLEX_SHA256_AMD64=30ad90eeaf6b476a84b4d9e959fd9e0f9cf9464708fd9355f30abf792169c322
ARG REFLEX_SHA256_ARM64=638f99652ae8cc014d18f568acd2d6ddd15bd6611ec22c5ae5e2aecfcec65357
RUN set -eu; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl; sum=$REFLEX_SHA256_AMD64 ;; \
      arm64) target=aarch64-unknown-linux-musl; sum=$REFLEX_SHA256_ARM64 ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    asset="reflex-$REFLEX_VERSION-$target.tar.gz"; \
    curl -fsSL -o "/tmp/$asset" "https://github.com/gist-rs/reflex/releases/download/$REFLEX_VERSION/$asset"; \
    echo "$sum  /tmp/$asset" | sha256sum -c -; \
    tar -xzf "/tmp/$asset" -C /usr/local/bin reflex; \
    rm "/tmp/$asset"; \
    reflex --version

# The host user, with the same uid/gid and home path, so mounted files keep their owner and absolute paths
# (symlinks in ~/.local/bin, transcripts keyed by working directory) resolve inside the container.
ARG MAP_UID=1000
ARG MAP_GID=1000
ARG MAP_USER=map
ARG MAP_HOME=/home/map
RUN set -eu; \
    if getent passwd "$MAP_UID" >/dev/null; then userdel -r "$(getent passwd "$MAP_UID" | cut -d: -f1)"; fi; \
    if getent group "$MAP_GID" >/dev/null; then groupdel "$(getent group "$MAP_GID" | cut -d: -f1)"; fi; \
    groupadd -g "$MAP_GID" "$MAP_USER"; \
    useradd -m -u "$MAP_UID" -g "$MAP_GID" -d "$MAP_HOME" -s /bin/bash "$MAP_USER"; \
    mkdir -p /data && chown "$MAP_UID:$MAP_GID" /data

COPY --from=backend /usr/local/bin/map /usr/local/bin/map
COPY --from=frontend /src/frontend/out /app/frontend/out

USER $MAP_UID:$MAP_GID
ENV HOME=$MAP_HOME LANG=C.UTF-8 TERM=xterm-256color
WORKDIR $MAP_HOME
EXPOSE 4400
# Any HTTP answer counts (with api_token set, /api/health answers 401 without the token).
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD curl -s -o /dev/null http://127.0.0.1:4400/api/health || exit 1
ENTRYPOINT ["tini", "--", "map"]
CMD ["serve", "-c", "/config/map.toml"]
