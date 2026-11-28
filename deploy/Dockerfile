FROM rust:1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/japka-rs/japka"
WORKDIR /app
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build --chmod=0555 /build/target/release/japka /usr/local/bin/japka
COPY --chown=10001:10001 public ./public
COPY LICENSE /usr/share/doc/japka/LICENSE
COPY notices/HYPER-LICENSE /usr/share/doc/japka/notices/HYPER-LICENSE
ENV DOCUMENT_ROOT=/app/public LISTEN=0.0.0.0:8080
EXPOSE 8080/tcp
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/japka"]
