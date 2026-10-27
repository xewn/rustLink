FROM rust:1.94-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends libudev-dev pkg-config cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN sh build.sh

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libudev1 curl && rm -rf /var/lib/apt/lists/* && useradd -u 10001 -m rustlink && mkdir /data && chown rustlink /data
COPY --from=build /src/target/release/rustlink /usr/local/bin/rustlink
USER rustlink
VOLUME ["/data"]
EXPOSE 8000
HEALTHCHECK --interval=15s --timeout=3s CMD curl --fail http://127.0.0.1:8000/api/health || exit 1
ENTRYPOINT ["rustlink"]
CMD ["--headless", "--host", "0.0.0.0", "--data-dir", "/data", "--port", "8000", "--strict-port", "--auto-start"]
