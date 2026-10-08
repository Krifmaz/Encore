ARG BASE_IMAGE=python:3.14-slim
FROM ${BASE_IMAGE} AS base

LABEL org.opencontainers.image.title="Encore" \
      org.opencontainers.image.description="Self-hosted music requests + library manager for Lidarr, slskd and yt-dlp" \
      org.opencontainers.image.source="https://github.com/Krifmaz/Encore" \
      org.opencontainers.image.url="https://github.com/Krifmaz/Encore"

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    ENCORE_CONFIG_DIR=/config \
    PUID=1000 \
    PGID=1000 \
    TZ=UTC

RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg libchromaprint-tools curl tini ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY requirements.txt .
RUN pip install -r requirements.txt

# Optional Qobuz/Tidal/Deezer downloader (uses your own subscription). Build with
# --build-arg INSTALL_STREAMRIP=false to leave it out.
ARG ENCORE_BUILD=dev
ENV ENCORE_BUILD=${ENCORE_BUILD}

ARG INSTALL_STREAMRIP=true
ARG STREAMRIP_REF=v2.4.9
RUN if [ "$INSTALL_STREAMRIP" = "true" ]; then \
      pip install "https://github.com/Stensel8/streamrip/archive/refs/tags/${STREAMRIP_REF}.zip" \
      || echo "WARNING: streamrip could not be installed"; \
    fi

COPY app ./app
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh && mkdir -p /config /music /downloads

VOLUME ["/config"]
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD python -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8080/api/health', timeout=4).status == 200 else 1)"

ENTRYPOINT ["/usr/bin/tini", "--", "/entrypoint.sh"]
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8080", "--proxy-headers", "--forwarded-allow-ips", "*"]


# ---------------------------------------------------------------------------------------------------
# All-in-one image (ghcr.io/krifmaz/encore:aio): slskd, Lidarr, Prowlarr, qBittorrent and SABnzbd inside
# Encore, configured and connected automatically. Enable with ENCORE_EMBEDDED=all.
#   docker build --target aio -t encore:aio .
# ---------------------------------------------------------------------------------------------------
FROM base AS aio
ARG TARGETARCH
ARG SLSKD_VERSION=0.26.0
ARG LIDARR_VERSION=3.1.0.4875
ARG PROWLARR_VERSION=2.3.0.5236
ARG QBITTORRENT_RELEASE=release-5.2.4_v2.0.15
ARG SABNZBD_VERSION=4.5.5
ENV ENCORE_EMBEDDED_ROOT=/opt/encore-services \
    UV_PYTHON_INSTALL_DIR=/opt/encore-services/python \
    DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=false

# .NET runtime deps (Lidarr/Prowlarr/slskd ship self-contained), SABnzbd's unpackers (unrar is in non-free)
RUN sed -i 's/^Components: main$/Components: main non-free/' /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && apt-get install -y --no-install-recommends libicu76 libsqlite3-0 unzip par2 7zip unrar \
 && rm -rf /var/lib/apt/lists/*

RUN set -eux; R=/opt/encore-services; mkdir -p $R/bin; \
    case "$TARGETARCH" in arm64) A=arm64; Q=aarch64;; *) A=x64; Q=x86_64;; esac; \
    curl -fsSL -o /tmp/slskd.zip "https://github.com/slskd/slskd/releases/download/${SLSKD_VERSION}/slskd-${SLSKD_VERSION}-linux-${A}.zip"; \
    mkdir -p $R/slskd && unzip -q /tmp/slskd.zip -d $R/slskd && rm /tmp/slskd.zip; \
    mkdir -p $R/lidarr && curl -fsSL "https://github.com/Lidarr/Lidarr/releases/download/v${LIDARR_VERSION}/Lidarr.master.${LIDARR_VERSION}.linux-core-${A}.tar.gz" | tar -xz -C $R/lidarr; \
    mkdir -p $R/prowlarr && curl -fsSL "https://github.com/Prowlarr/Prowlarr/releases/download/v${PROWLARR_VERSION}/Prowlarr.master.${PROWLARR_VERSION}.linux-core-${A}.tar.gz" | tar -xz -C $R/prowlarr; \
    curl -fsSL -o $R/bin/qbittorrent-nox "https://github.com/userdocs/qbittorrent-nox-static/releases/download/${QBITTORRENT_RELEASE}/${Q}-qbittorrent-nox"; \
    chmod +x $R/bin/qbittorrent-nox $R/slskd/slskd; \
    mkdir -p $R/sabnzbd && curl -fsSL "https://github.com/sabnzbd/sabnzbd/releases/download/${SABNZBD_VERSION}/SABnzbd-${SABNZBD_VERSION}-src.tar.gz" | tar -xz -C $R/sabnzbd --strip-components=1; \
    pip install uv && uv venv --python 3.13 $R/sabnzbd/venv && uv pip install --python $R/sabnzbd/venv/bin/python -r $R/sabnzbd/requirements.txt; \
    chmod -R a+rX $R

ENV ENCORE_EMBEDDED=all
# web UIs are proxied through Encore (8080); these are the peer ports for incoming connections
EXPOSE 50300 6881 6881/udp

# the slim image stays the default build target
FROM base AS default
