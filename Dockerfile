# ---------- 构建阶段 ----------
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

# 先编译依赖，利用 Docker 层缓存
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && cargo build --release

COPY src ./src
COPY static ./static
RUN touch src/main.rs && cargo build --release

# ---------- 运行阶段 ----------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/file-renamer /usr/local/bin/file-renamer
COPY --from=builder /app/static ./static

ENV BIND_ADDR=0.0.0.0:8999 \
    DATA_ROOT=/data \
    STATIC_DIR=/app/static

RUN mkdir -p /data

EXPOSE 8999
CMD ["file-renamer"]
