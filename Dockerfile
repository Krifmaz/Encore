# Encore (server + web app, one Rust binary). Builds for linux/amd64 and linux/arm64; the arm64 binary is
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
# the server is a Cargo workspace: cargo reads every member before it builds one, so all of them come in – the
# crates (crates/*), the speed bench Cargo.toml names (benches/) and the encore binary itself (src/)
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/crates ./crates
COPY server/benches ./benches
COPY server/src ./src
RUN case "$TARGETARCH" in arm64) T=aarch64-unknown-linux-gnu ;; *) T=x86_64-unknown-linux-gnu ;; esac \
 && { [ -n "$ENCORE_VERSION" ] || unset ENCORE_VERSION; } \
 && cargo build --release --locked --target "$T" && cp "target/$T/release/encore" /encore

# The web app in Rust/WebAssembly (web-rs): built once on the build machine with trunk, served at / by the server.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS ui
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates brotli && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && curl -fsSL https://github.com/trunk-rs/trunk/releases/download/v0.21.14/trunk-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C /usr/local/bin trunk
WORKDIR /ui
COPY web-rs ./
# every text file of the build also as <file>.br (brotli, best) and <file>.gz (gzip, best) next to it: the server
# sends the one the browser takes as it is, so nothing is compressed per request and the .wasm comes much smaller
RUN trunk build --release \
 && find dist -type f \( -name '*.js' -o -name '*.wasm' -o -name '*.css' -o -name '*.html' -o -name '*.svg' \
      -o -name '*.webmanifest' -o -name '*.json' \) -exec brotli -f -k -q 11 {} + -exec gzip -f -k -n -9 {} + \
 && find dist -name '*.br' | grep -q . && find dist -name '*.gz' | grep -q .

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
ARG STREAMRIP_REF=v2.4.10
# The streamrip fork needs Python 3.14+, newer than Debian's python3, so uv fetches a standalone 3.14 for the
# tools' venv. No silent fallback to PyPI's streamrip 2.1.0: its Tidal client was revoked, so a failed install
# must fail the build instead of shipping a streamrip that cannot sign in to Tidal.
ENV UV_PYTHON_INSTALL_DIR=/opt/python UV_NO_CACHE=1
RUN python3 -m venv /opt/uv && /opt/uv/bin/pip install --no-cache-dir uv \
 && /opt/uv/bin/uv venv --seed --python 3.14 /opt/tools \
 && /opt/uv/bin/uv pip install --python /opt/tools/bin/python "yt-dlp[default]" \
 && if [ "$INSTALL_STREAMRIP" = "true" ]; then \
      /opt/uv/bin/uv pip install --python /opt/tools/bin/python "https://github.com/Stensel8/streamrip/archive/refs/tags/${STREAMRIP_REF}.zip" \
      && { /opt/tools/bin/streamrip --version 2>/dev/null || /opt/tools/bin/rip --version; } | tee /dev/stderr | grep -q "${STREAMRIP_REF#v}" \
      || { echo "streamrip ${STREAMRIP_REF} failed to install"; exit 1; }; \
    fi \
 && rm -rf /opt/uv
ENV PATH=/opt/tools/bin:$PATH \
    ENCORE_CONFIG_DIR=/config ENCORE_STATIC_DIR=/app/static ENCORE_PORT=8080 \
    PUID=1000 PGID=1000 TZ=UTC
COPY --from=build /encore /usr/local/bin/encore
# the Rust UI (trunk's dist: index.html, the hashed .js/.wasm, css, icons, manifest, share.html, sw.js) is the website
COPY --from=ui /ui/dist /app/static
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
