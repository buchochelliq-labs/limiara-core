FROM rust:1.95.0-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p api
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 appuser
USER appuser
COPY --from=build /src/target/release/api /usr/local/bin/api
COPY --from=build /src/config/default.yml /config/default.yml
COPY --from=build /src/plugins /plugins
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/api"]
