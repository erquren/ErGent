# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim AS web
WORKDIR /src
RUN npm install --global pnpm@10.17.1
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml .npmrc ./
COPY patches ./patches
COPY apps/web ./apps/web
COPY packages ./packages
RUN pnpm install --frozen-lockfile && pnpm build

# Build on the target architecture: CI uses native amd64 and arm64 runners.
FROM rust:1.98.1-bookworm AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps/server ./apps/server
COPY apps/agent ./apps/agent
COPY crates ./crates
COPY migrations ./migrations
RUN cargo build --release --locked -p ergent-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 ergent \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /data --shell /usr/sbin/nologin ergent \
    && install -d -m 0700 -o 10001 -g 10001 /data
WORKDIR /app
COPY --from=server /src/target/release/ergent-server /app/ergent-server
COPY --from=web /src/apps/web/dist /app/web
COPY --chmod=0555 scripts/docker-healthcheck.sh /app/docker-healthcheck
ENV ERGENT_DATABASE=/data/server.db \
    ERGENT_LISTEN=0.0.0.0:7777 \
    ERGENT_WEB=/app/web \
    ERGENT_TLS_MODE=direct
USER 10001:10001
EXPOSE 7777
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/app/docker-healthcheck"]
ENTRYPOINT ["/app/ergent-server"]
CMD ["run"]
