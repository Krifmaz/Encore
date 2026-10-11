# Encore (Rust server + web app). Builds for linux/amd64 and linux/arm64; the arm64 binary is
# cross-compiled on the build machine (fast), not emulated.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS build
ARG TARGETARCH
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
      && rm -rf /var/lib/apt/lists/* && rustup target add aarch64-unknown-linux-gnu; \
    fi
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
ARG ENCORE_VERSION=
WORKDIR /src
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/src ./src
RUN case "$TARGETARCH" in arm64) T=aarch64-unknown-linux-gnu ;; *) T=x86_64-unknown-linux-gnu ;; esac \
 && { [ -n "$ENCORE_VERSION" ] || unset ENCORE_VERSION; } \
 && cargo build --release --locked --target "$T" && cp "target/$T/release/encore" /encore

# The web app in Rust/WebAssembly (web-rs): built once on the build machine, served at /next/ beside the JS app.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS ui
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C /usr/local/bin trunk
WORKDIR /ui
COPY web-rs ./
RUN trunk build --release

FROM debian:trixie-slim
LABEL org.opencontainers.image.title="Encore" \
      org.opencontainers.image.description="Self-hosted music requests + library manager" \
      org.opencontainers.image.source="https://github.com/Krifmaz/Encore" \
      org.opencontainers.image.url="https://github.com/Krifmaz/Encore"
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg libchromaprint-tools ca-certificates curl tzdata tini python3 python3-venv nodejs \
 && rm -rf /var/lib/apt/lists/*
# download tools Encore runs as programs: yt-dlp (YouTube, SoundCloud, Bandcamp …) and, optionally,
# streamrip (Qobuz/Tidal/Deezer with your own subscription). Build with --build-arg INSTALL_STREAMRIP=false to skip it.
ARG INSTALL_STREAMRIP=true
ARG STREAMRIP_REF=v2.4.9
RUN python3 -m venv /opt/tools && /opt/tools/bin/pip install --no-cache-dir "yt-dlp[default]" \
 && if [ "$INSTALL_STREAMRIP" = "true" ]; then \
      /opt/tools/bin/pip install --no-cache-dir "https://github.com/Stensel8/streamrip/archive/refs/tags/${STREAMRIP_REF}.zip" \
      || /opt/tools/bin/pip install --no-cache-dir streamrip; \
      /opt/tools/bin/rip --version || { echo "streamrip failed to install"; exit 1; }; \
    fi
ENV PATH=/opt/tools/bin:$PATH \
    ENCORE_CONFIG_DIR=/config ENCORE_STATIC_DIR=/app/static ENCORE_PORT=8080 \
    PUID=1000 PGID=1000 TZ=UTC
COPY --from=build /encore /usr/local/bin/encore
COPY web /app/static
COPY --from=ui /ui/dist /app/static/next
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh && mkdir -p /config /music /downloads
ARG ENCORE_BUILD=dev
ARG ENCORE_VERSION=
ENV ENCORE_BUILD=${ENCORE_BUILD}
LABEL org.opencontainers.image.version="${ENCORE_VERSION}"
VOLUME /config
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/entrypoint.sh"]
CMD ["encore"]
