ARG BASE_IMAGE=python:3.14-slim
FROM ${BASE_IMAGE}

LABEL org.opencontainers.image.title="Encore" \
      org.opencontainers.image.description="Self-hosted music requests + library manager for Lidarr, slskd and yt-dlp" \
      org.opencontainers.image.source="https://github.com/krifmaz/test"

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    ENCORE_CONFIG_DIR=/config \
    PUID=1000 \
    PGID=1000 \
    TZ=UTC

RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg libchromaprint-tools tini ca-certificates tzdata \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY requirements.txt .
RUN pip install -r requirements.txt

# Optional Qobuz/Tidal/Deezer downloader (uses your own subscription). Build with
# --build-arg INSTALL_STREAMRIP=false to leave it out.
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
