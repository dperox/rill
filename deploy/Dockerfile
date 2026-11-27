FROM rust:1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/dperox/rill"
WORKDIR /app
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /build/target/release/rill /usr/local/bin/rill
COPY public ./public
COPY LICENSE /usr/share/doc/rill/LICENSE
COPY notices/HYPER-LICENSE /usr/share/doc/rill/notices/HYPER-LICENSE
ENV DOCUMENT_ROOT=/app/public LISTEN=0.0.0.0:8080
EXPOSE 8080/tcp
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/rill"]
