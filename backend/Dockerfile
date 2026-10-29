# Tasker backend — multi-stage production build.
# The binary is statically linked against musl for a minimal runtime image.

FROM lukemathwalker/cargo-chef:latest-rust-1.82-slim AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies only first for layer caching.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
ENV SQLX_OFFLINE=true
RUN cargo build --release --bin tasker

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
# Non-root runtime user.
RUN useradd --system --home /var/lib/tasker --shell /usr/sbin/nologin tasker \
 && mkdir -p /var/lib/tasker && chown tasker:tasker /var/lib/tasker
COPY --from=builder /app/target/release/tasker /usr/local/bin/tasker

USER tasker
WORKDIR /var/lib/tasker
ENV TASKER_HTTP_HOST=0.0.0.0 \
    TASKER_HTTP_PORT=8080 \
    TASKER_ENV=production
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -fsS http://localhost:8080/api/v1/health/ready || exit 1
ENTRYPOINT ["/usr/local/bin/tasker"]
CMD ["serve"]
