FROM rust:1.98-slim-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# reqwest is built without TLS, so no CA bundle is needed; distroless/cc provides glibc.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/img-proxy /usr/local/bin/img-proxy

ENV LISTEN_ADDR=0.0.0.0:8080 \
    RUST_LOG=info

EXPOSE 8080

USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/img-proxy"]
