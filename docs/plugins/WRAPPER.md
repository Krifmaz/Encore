# The Apple Music wrapper plugin

gamdl downloads Apple Music as AAC 256k with your media-user-token alone. **ALAC lossless and Dolby Atmos need a
wrapper**: a small service signed in to your Apple Music account that decrypts Apple's FairPlay streams for
gamdl. The one gamdl 3.9 speaks to is [wrapper-v2](https://github.com/glomatico/wrapper-v2) (API 0.0.2). The
**Apple Music wrapper** plugin (Settings → Plugins) runs one for you, signs it in and points gamdl at it.

Written for Encore 0.37. The code is `server/crates/requests/src/wrapper/` (the API routes are
`server/crates/api/src/routes/wrapper.rs`, the card's block `web-rs/src/settings/wrapper.rs`).

## What you need

- **An Apple Music subscription.** Without one nothing downloads, whatever the wrapper says.
- **An Apple ID with two-factor authentication** (every Apple ID today). Apple sends a six-digit code to your
  iPhone, iPad or Mac when the wrapper signs in; you type it on the card.
- For the wrapper Encore runs:
  - **The Docker socket mounted into Encore's container**, as "Update now" uses it:
    `- /var/run/docker.sock:/var/run/docker.sock` under `volumes:` (docker-compose.yml has it commented out).
    Encore must be allowed to use it: `PGID` set to the socket's group, or `PUID=0`. The socket gives Encore
    control of Docker on that host; mount it only if you accept that.
  - **Apple Music for Android, version 3.6.0-beta (build 1109)**, as an `.apk` or the `.apkm` bundle APKMirror
    hands out. wrapper-v2 ships no Apple code, and nobody may publish an image with it in, so you bring the app
    and Encore builds the image on your Docker. Other versions are refused before the build: the libraries are
    checked against the SHA-256 sums wrapper-v2 pins (`LIBS_VERSION.json`).
  - **An x86_64 or arm64 server.** The image's build stage is x86_64 only (Google publishes the Android NDK for
    x86_64 hosts); on an arm64 server Docker needs QEMU's binfmt handlers to build it
    (`docker run --privileged --rm tonistiigi/binfmt --install amd64`), or build it elsewhere (below).
  - About **5 GB of free disk** for the build (the NDK, Rust, the compile) and 10 to 30 minutes the first time.
    The finished image is a few hundred MB.

## Setting it up (Encore runs it)

1. Settings → Plugins → **Apple Music wrapper** → Install. "Wrapper" stays on **Encore runs it (Docker socket)**.
2. **Upload app**: the Apple Music `.apk` or `.apkm`.
3. **Set up**. In the background Encore downloads wrapper-v2's source (the commit it was written for), takes the
   Apple libraries out of the app, adds the Android system files from the source, builds the image
   `encore-wrapper-v2:<commit>` through Docker, and starts it as the container `encore-wrapper`. The card shows
   each step; every build line goes to the plugin's log (Logs on the card).
4. **Sign in** with your Apple ID and password, then type the code Apple sends. The password goes to the wrapper
   only: Encore keeps it nowhere, not in its settings, not in a log.
5. With **Point gamdl at this wrapper** on (the default), gamdl's **Wrapper URL** and **Wrapper decrypt address**
   are filled in when the wrapper starts and when it signs in. Otherwise press **Use with gamdl**. Then choose
   **ALAC lossless** under gamdl's Format, or use Get Dolby Atmos version (Settings → Library → Surround).

The Apple session is kept in the Docker volume `encore-wrapper-data`, so the wrapper stays signed in across
restarts, image rebuilds and Encore updates. Sign out on the card ends it.

### Where the container runs

The container gets what wrapper-v2's compose file gives it: the capabilities `SYS_ADMIN`, `SYS_CHROOT` and
`SYS_PTRACE` and no AppArmor profile (it sets up a chroot of Android's libraries). Encore's own container keeps
none of these. Where it goes depends on Encore's container:

| Encore runs | The wrapper | gamdl's Wrapper URL |
|---|---|---|
| on a Docker network (compose's `music`) | joins that network, named `encore-wrapper`; no port on the host | `http://encore-wrapper`, decrypt `encore-wrapper:10020` |
| on Docker's default bridge | joins the bridge | its address there (refreshed at every Start) |
| on the host's network, or outside Docker | its ports published on **127.0.0.1 only** | `http://127.0.0.1:30080`, decrypt `127.0.0.1:10020` |

Docker restarts the container with the host (`unless-stopped`). Stop and Start on the card stop and start it;
switching the plugin off stops it; Remove removes the container (the image and the session's volume stay: remove
them with `docker image rm encore-wrapper-v2:<commit>` and `docker volume rm encore-wrapper-data` if you want).
A container named `encore-wrapper` that Encore did not make is left alone.

### Building it yourself instead

When the build cannot run through Encore (Docker's API builder refused, an arm64 host without QEMU), build the
image anywhere with wrapper-v2's own instructions (`tools/extract-libs.sh`, `tools/stage-system.sh`,
`docker compose build`), push or load it into the Docker Encore uses, and put its name under **Image to run
instead** (advanced). Encore then pulls and runs that image, with no app upload and no build. The source Encore
staged last is kept in `/config/plugins/wrapper/data/build/wrapper-v2-<commit>/` and builds there with `docker build .`.

Encore drives Docker's Engine API builder (`version=1`), which does not know BuildKit's `RUN --mount=type=cache`:
Encore takes those flags out of the Dockerfile (they only keep apt's downloads between builds).

## A wrapper you run (external)

Choose **A wrapper I run** and enter its control address (for example `http://wrapper-v2:80`). Its decrypt port,
10020, must be reachable on the same host. The card then signs it in, shows its state, tests it and fills in
gamdl's Wrapper URL the same way; Set up, Start, Stop and the container log are for the wrapper Encore runs.

## What the card shows and Test checks

- Status: the setup's step while one runs; otherwise what is missing (the socket, the app, Set up, Start) or
  the account ("Signed in as you@icloud.com · storefront us").
- **Test**: `/health` answers, `/me` speaks API 0.0.2 (the one gamdl needs), Apple's libraries loaded
  (`runtime.loader_ok`: false when the image was built from another app version or the container lacks its
  privileges), the account is signed in, and whether gamdl uses this wrapper.
- **Wrapper log**: the container's own log (Docker's). The plugin's **Logs** has Encore's lines: the upload,
  every build line, start, stop and each sign-in step's outcome (never a password or a token).

## The API

Admins only, with the CSRF header (`X-Requested-With: encore`):

| Route | What it does |
|---|---|
| `GET /api/system/wrapper` | The card's state: mode, Docker, the uploaded app, the image, the container, the setup job, the account (never a token), gamdl |
| `POST /api/system/wrapper/app` | Upload the app (multipart, one file, up to 2 GB) |
| `POST /api/system/wrapper/set-up` | `{rebuild?}`: download, stage, build (or pull) and start, in the background |
| `POST /api/system/wrapper/start`, `/stop` | Start or stop the container |
| `POST /api/system/wrapper/sign-in` | `{apple_id, password}`: the first step; answers `{state: "awaiting_2fa" \| "authenticated" \| "failed", message}` |
| `POST /api/system/wrapper/sign-in/code` | `{code}`: the six digits Apple sent |
| `DELETE /api/system/wrapper/sign-in` | Sign out (or stop a sign-in half way) |
| `POST /api/system/wrapper/gamdl` | Point gamdl at this wrapper |
| `GET /api/system/wrapper/logs?lines=300` | The container's log |

## Limits

- Apple Music only: the wrapper decrypts for gamdl, it does not search or download by itself.
- One Apple ID per wrapper. Apple may ask for the code again after a while or after a password change: sign in
  again on the card.
- wrapper-v2 is research-grade software, pinned here at one commit; Apple can change its service at any time,
  and a newer Apple Music app needs a newer wrapper-v2 (a new Encore release moves the pin).
- Downloading may break Apple's terms of service; use it for music your subscription lets you play.
