FROM rust:1.95.0-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p worker
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 appuser
USER appuser
COPY --from=build /src/target/release/worker /usr/local/bin/worker
COPY --from=build /src/config/default.yml /config/default.yml
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/worker"]
