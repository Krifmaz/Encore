# Encore 2 (Rust server). Build from the repository root:
#   docker build -f server/Dockerfile -t encore:rust .
FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/src ./src
RUN cargo build --release --locked

FROM debian:trixie-slim
LABEL org.opencontainers.image.title="Encore" \
      org.opencontainers.image.description="Self-hosted music requests + library manager (Rust server)" \
      org.opencontainers.image.source="https://github.com/Krifmaz/Encore"
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg libchromaprint-tools ca-certificates tzdata tini python3 python3-venv \
 && rm -rf /var/lib/apt/lists/*
# download tools Encore runs as programs: yt-dlp (YouTube, SoundCloud, Bandcamp …) and, optionally,
# streamrip (Qobuz/Tidal/Deezer with your own subscription). Build with --build-arg INSTALL_STREAMRIP=false to skip it.
ARG INSTALL_STREAMRIP=true
ARG STREAMRIP_REF=v2.4.9
RUN python3 -m venv /opt/tools && /opt/tools/bin/pip install --no-cache-dir "yt-dlp[default]" \
 && if [ "$INSTALL_STREAMRIP" = "true" ]; then \
      /opt/tools/bin/pip install --no-cache-dir "https://github.com/Stensel8/streamrip/archive/refs/tags/${STREAMRIP_REF}.zip" \
      || echo "WARNING: streamrip could not be installed"; \
    fi
ENV PATH=/opt/tools/bin:$PATH
ENV ENCORE_CONFIG_DIR=/config ENCORE_STATIC_DIR=/app/static ENCORE_PORT=8080 TZ=UTC
COPY --from=build /src/target/release/encore /usr/local/bin/encore
COPY app/static /app/static
ARG ENCORE_BUILD=dev
ENV ENCORE_BUILD=${ENCORE_BUILD}
VOLUME /config
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s CMD ["/usr/bin/env", "bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8080"]
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["encore"]
