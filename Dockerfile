FROM rust:latest AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY static ./static
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && \
    apt-get install -y libssl3 ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/webhook-forwarder .
COPY static ./static
RUN mkdir -p /data
ENV DATA_DIR=/data
EXPOSE 8080
CMD ["./webhook-forwarder"]
