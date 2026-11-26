# Build the recvrd daemon and the web UI in separate stages.
# The runtime stage carries the daemon, yt-dlp, and ffmpeg only.
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

# Stage 2: build the Rust daemon and the terminal client.
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

# Stage 3: run the daemon in a development container.
# This stage adds Node from the web stage, so it can rebuild the web UI.
# Compose uses this stage for development only.
FROM build AS dev
COPY --from=web /usr/local /usr/local
WORKDIR /app
ENTRYPOINT ["cargo", "run", "-p", "recvrd", "--"]
CMD ["serve"]

# Stage 4: run the daemon.
FROM debian:bookworm-slim AS runtime
ARG YT_DLP_VERSION=2026.11.23
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    ffmpeg \
    libdbus-1-3 \
    libpipewire-0.3-0 \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL -o /tmp/yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP_VERSION}/yt-dlp" \
    && curl -fsSL -o /tmp/SHA2-256SUMS "https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP_VERSION}/SHA2-256SUMS" \
    && cd /tmp && grep ' yt-dlp$' SHA2-256SUMS | sha256sum -c - \
    && install -m 0755 /tmp/yt-dlp /usr/local/bin/yt-dlp \
    && rm /tmp/yt-dlp /tmp/SHA2-256SUMS \
    && groupadd -r recvrd --gid 1000 \
    && useradd -r -m -g recvrd --uid 1000 --home-dir /home/recvrd --shell /usr/sbin/nologin recvrd \
    && mkdir -p /home/recvrd/.config/recvrd /home/recvrd/.cache/recvrd /home/recvrd/.local/state/recvrd /home/recvrd/.local/share/recvrd \
    && chown -R recvrd:recvrd /home/recvrd
COPY --from=build /app/target/release/recvrd /usr/local/bin/recvrd
COPY --from=build /app/target/release/ytermusicd /usr/local/bin/ytermusicd
COPY --from=build /app/target/release/recvc /usr/local/bin/recvc
COPY --from=build /app/target/release/ytmc /usr/local/bin/ytmc
COPY config.example.toml /usr/local/share/doc/recvrd/config.example.toml
USER recvrd
WORKDIR /home/recvrd
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${RECVRD_PORT:-8080}/api/status"
ENTRYPOINT ["/usr/local/bin/recvrd"]
CMD ["serve"]
