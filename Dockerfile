# Build the web UI, the daemon, and the terminal client in separate stages.
# The runtime stage carries the binaries, yt-dlp, ffmpeg, and the runtime libraries.
# It carries no Rust toolchain, no Node toolchain, and no source code.
# TLS uses rustls. The runtime needs ca-certificates, not libssl.
# Storage uses flat files. The runtime needs no SQLite library.

# Stage 1: build the web UI.
FROM node:22-bookworm-slim AS web
WORKDIR /app
COPY crates/recvrd-web/package.json crates/recvrd-web/package-lock.json ./
RUN npm ci
COPY crates/recvrd-web/ ./
RUN npm run build

# Stage 2: fetch the standalone yt-dlp binary for the target architecture.
# The standalone asset needs no Python. The Python zipapp does.
FROM debian:bookworm-slim AS ytdlp
ARG TARGETARCH
ARG YT_DLP_VERSION=2026.11.23
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && arch="${TARGETARCH}" \
    && if [ -z "$arch" ]; then arch="$(dpkg --print-architecture)"; fi \
    && case "$arch" in \
         amd64) asset="yt-dlp_linux" ;; \
         arm64) asset="yt-dlp_linux_aarch64" ;; \
         *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
       esac \
    && curl -fsSL -o "/tmp/$asset" "https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP_VERSION}/$asset" \
    && curl -fsSL -o /tmp/SHA2-256SUMS "https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP_VERSION}/SHA2-256SUMS" \
    && cd /tmp \
    && grep " $asset$" SHA2-256SUMS | sha256sum -c - \
    && install -m 0755 "$asset" /usr/local/bin/yt-dlp \
    && /usr/local/bin/yt-dlp --version

# Stage 3: copy the PipeWire client configuration.
# The library package omits client.conf and the context fails without it.
FROM debian:bookworm-slim AS pipewire-config
RUN apt-get update && apt-get install -y --no-install-recommends \
    pipewire-bin \
    && rm -rf /var/lib/apt/lists/*

# Stage 4: build the Rust daemon and the terminal client.
FROM rust:1.99-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libclang-dev \
    libpipewire-0.3-dev \
    libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=web /app/dist ./crates/recvrd-web/dist
RUN cargo build --locked --release -p recvrd -p recvc

# Stage 5: run the daemon in a development container.
# This stage adds Node, yt-dlp, and ffmpeg to build the web UI and download tracks.
# Compose uses this stage for development only.
FROM build AS dev
ARG UID=1000
ARG GID=1000
COPY --from=web /usr/local /usr/local
COPY --from=ytdlp /usr/local/bin/yt-dlp /usr/local/bin/yt-dlp
COPY --from=pipewire-config /usr/share/pipewire/client.conf /usr/share/pipewire/client.conf
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    ffmpeg \
    libpipewire-0.3-modules \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --non-unique --gid "$GID" recvrd \
    && useradd --non-unique --uid "$UID" --gid recvrd --home-dir /home/recvrd --no-create-home --shell /usr/sbin/nologin recvrd \
    && install -d -o recvrd -g recvrd -m 0755 \
         /home/recvrd \
         /home/recvrd/.cargo \
         /home/recvrd/.config/recvrd \
         /home/recvrd/.cache/recvrd \
         /home/recvrd/.local/state/recvrd \
         /home/recvrd/.local/share/recvrd \
         /cargo-target \
    && install -d -o recvrd -g recvrd -m 0700 "/run/user/$UID"
ENV HOME=/home/recvrd
ENV CARGO_HOME=/home/recvrd/.cargo
ENV CARGO_TARGET_DIR=/cargo-target
WORKDIR /app
HEALTHCHECK --interval=30s --timeout=5s --start-period=600s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${RECVRD_PORT:-8080}/api/status"
ENTRYPOINT ["cargo", "run", "-p", "recvrd", "--bin", "recvrd", "--"]
CMD ["serve"]

# Stage 6: run the daemon.
FROM debian:bookworm-slim AS runtime
ARG UID=1000
ARG GID=1000
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    ffmpeg \
    libdbus-1-3 \
    libpipewire-0.3-0 \
    libpipewire-0.3-modules \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --non-unique --gid "$GID" recvrd \
    && useradd --non-unique --uid "$UID" --gid recvrd --home-dir /home/recvrd --no-create-home --shell /usr/sbin/nologin recvrd \
    && install -d -o recvrd -g recvrd -m 0755 \
         /home/recvrd \
         /home/recvrd/.config/recvrd \
         /home/recvrd/.cache/recvrd \
         /home/recvrd/.local/state/recvrd \
         /home/recvrd/.local/share/recvrd \
    && install -d -o recvrd -g recvrd -m 0700 "/run/user/$UID"
COPY --from=ytdlp /usr/local/bin/yt-dlp /usr/local/bin/yt-dlp
COPY --from=pipewire-config /usr/share/pipewire/client.conf /usr/share/pipewire/client.conf
COPY --from=build /app/target/release/recvrd /usr/local/bin/recvrd
COPY --from=build /app/target/release/ytermusicd /usr/local/bin/ytermusicd
COPY --from=build /app/target/release/recvc /usr/local/bin/recvc
COPY --from=build /app/target/release/ytmc /usr/local/bin/ytmc
COPY config.example.toml /usr/local/share/doc/recvrd/config.example.toml
ENV HOME=/home/recvrd
USER recvrd
WORKDIR /home/recvrd
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${RECVRD_PORT:-8080}/api/status"
ENTRYPOINT ["/usr/local/bin/recvrd"]
CMD ["serve"]
