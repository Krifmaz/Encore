# The plugin sandbox

How Encore runs the plugins you add (uploaded, or installed from a URL), what each sandbox level hides from
them, and what a Docker host needs for the stricter level. Written for Encore 0.32.0. The process contract
is in [CONTRACT.md](CONTRACT.md); the manifest's `[permissions]` table in
[docs/contracts/plugin-manifest.md](../contracts/plugin-manifest.md#permissions).

The sandbox is about **plugin programs**: the plugins you add, which Encore starts as programs of their own
(protocol 1). The catalog's plugins are Encore's own code; the programs they install and start (yt-dlp,
streamrip, slskd, Cadenza, the Translator's llama-server) run as Encore's own helpers, outside bubblewrap, as
they did before 0.32.

## The two levels

Encore finds out which level the host allows once after each start, and every plugin's card shows it in the
sandbox line, with bubblewrap's own words when it is not full isolation.

| | Full isolation | Limits only |
|---|---|---|
| When | `bwrap` (bubblewrap) can make namespaces here | Not: bubblewrap is missing, or the host or the container refuses it |
| Sandbox line | Full isolation | Limits only, see the compose option |
| Files it can read | The system folders (read-only), its own folder, nothing else | Everything Encore's user can read |
| Config folder, database, other plugins | Not there | Readable |
| Your music | Only when its manifest asks to read the library, and then read-only | Readable |
| Network | Only when its manifest asks for it | The server's network |
| Processes it can see | Its own | Every process of Encore's user (Encore itself is protected, see below) |
| Limits, environment, priority | Both levels, below | Both levels, below |

**Full isolation** runs the plugin under bubblewrap in namespaces of its own: user, mount, process (pid),
IPC, host name (UTS), cgroup and, unless its manifest asks for the network, network. Inside it sees:

- the system folders read-only: `/usr` and the `/bin`, `/sbin`, `/lib`, `/lib64` links into it, the parts of
  `/etc` that programs need to start, resolve names and check certificates (`/etc/ssl`,
  `/etc/ca-certificates`, `/etc/pki`, `/etc/resolv.conf`, `/etc/hosts`, `/etc/nsswitch.conf`,
  `/etc/host.conf`, `/etc/alternatives`, `/etc/ld.so.cache`, `/etc/ld.so.conf`, `/etc/localtime`,
  `/etc/mime.types`) and `/opt/python` (the image's Python 3.14);
- its installed version read-only (`<config>/plugins/<id>/<version>/`), and its data folder, home and
  temporary folder writable (`data/`, `home/`, `tmp/` beside it);
- the request's download folder, writable, while a download runs;
- the library, read-only, only when the manifest asks for it
  (`paths = [{ setting = "library_path", access = "read" }]`, which its card shows as "Reads your library
  (read only)");
- a private `/tmp`, a minimal `/dev`, and a `/proc` that shows only its own processes (or no `/proc` at all
  where Docker does not allow one, see [/proc](#proc));

and nothing else of the host: not the config folder, not the database, not the other plugins, not the Docker
socket when it is mounted. It ends when Encore ends.

**Limits only** sets the rules below and nothing more. The plugin runs as Encore's user, so it can read
what Encore can read: the config folder with the database and saved secrets, the library, the other
plugins. Encore makes itself non-dumpable at this level, so a plugin cannot read Encore's own memory or
environment through `/proc`. The programs Encore starts itself (yt-dlp, streamrip, slskd, Cadenza, the
translator) are ordinary processes of the same user: they get Encore's environment without Encore's settings
(no `ENCORE_*` variable and none that a setting reads, such as passwords and API keys), but a plugin at this
level can read the rest of their environment and their command lines while they run. Install a plugin at this
level only when you trust its author as you trust Encore.

**Both levels**:

- **Limits** from the manifest's `[permissions]`, else the defaults: memory (address space) 2048 MB,
  processor time as long as the command's time limit, 64 more processes, 1024 open files, no core dumps.
- **No new privileges**: a program the plugin starts can never gain rights (set-uid programs do not work).
- **A session of its own**, without a terminal; when Encore stops a command, it stops every process the
  plugin started (SIGTERM, then SIGKILL 10 seconds later).
- **Low priority**: nice 10 and idle disk priority, so a plugin never slows down playback or the library.
- **A cleared environment**: only the variables [CONTRACT.md](CONTRACT.md#environment) lists, with a
  private `HOME`, `TMPDIR` and XDG folders under the plugin's folder. Settings and secrets reach it on
  stdin only.
- **Only Encore's files out**: a plugin delivers files into the download folder; Encore checks, tags and
  imports them.

A third-party plugin always gets these rules; its card says it is third-party and what its sandbox level
means. The catalog's plugins show no such warning.

## How Encore finds the level

Once after each start, Encore runs bubblewrap exactly as it would run a plugin (the same arguments, the same
limits) around `/bin/true`: first with a `/proc` of its own, then without one. The first that works is the
level; when neither does, it is limits only, and the last line bubblewrap printed is kept as the reason. The
server's log says which at start, for example `plugins: full isolation (bubblewrap)`,
`plugins: full isolation (bubblewrap, no /proc)` or `plugins: limits only (bwrap: No permissions to create
new namespace, …)`.

## In Docker

Encore's image has bubblewrap. It is not set-uid there, and Encore starts it with no new privileges, so
bubblewrap works only where an ordinary user may make **user namespaces**. Run under `strace`, the probe makes
these calls: `clone` with the namespace flags, `unshare`, `mount` (about 30), `umount2` and `pivot_root`.
With Docker's defaults, two things stop it:

1. **Docker's default seccomp profile** allows `clone` and `unshare` with namespace flags, `mount` and
   `umount2` only to a container that has `CAP_SYS_ADMIN` (moby's `profiles/seccomp/default.json`). Encore's
   container does not have it, so bubblewrap stops at once. Its words, shown under the sandbox line:
   `No permissions to create new namespace, likely because the kernel does not allow non-privileged user
   namespaces.` In a container that is nearly always the seccomp profile, not the kernel.
2. **Docker's default AppArmor profile** (`docker-default`, used on hosts with AppArmor: Ubuntu, Debian,
   openSUSE) has `deny mount`, so bubblewrap stops at its first mount: `Failed to make / slave: Permission
   denied`. Hosts without AppArmor (Fedora, RHEL, Unraid) do not have this one.

### The compose option

To give plugins full isolation, let the container make namespaces and mounts:

```yaml
services:
  encore:
    image: ghcr.io/krifmaz/encore:latest
    security_opt:
      - seccomp=unconfined     # Docker's seccomp profile allows namespaces only with CAP_SYS_ADMIN
      - apparmor=unconfined    # only on hosts with AppArmor (Ubuntu, Debian): its profile forbids the mounts
```

With `docker run`: `--security-opt seccomp=unconfined --security-opt apparmor=unconfined`. On Unraid, put
the same in the container's *Extra Parameters* (advanced view). Podman takes the same two options. Recreate
the container (`docker compose up -d`), open Settings → Plugins and check that the sandbox line says
**Full isolation**.

What it costs: these switch off two of Docker's protections for the whole container, Encore and every program
it runs included. The container then relies on the kernel's own checks and on Encore running as `PUID` with
no capabilities. User namespaces themselves have been the way into several kernel bugs over the years, so keep
the host's kernel up to date. What it gives: a plugin you add sees only its own folders, not your config
folder, database, secrets or music.

Do not use `cap_add: [SYS_ADMIN]` or `privileged: true` for this. `CAP_SYS_ADMIN` would also get the namespace
calls past Docker's seccomp profile, but it gives the container's root user far more than bubblewrap needs
(mounting file systems among it), and Encore's own user drops it anyway.

**Which to choose.** If you only use the catalog's plugins, limits only is fine: their programs run outside
bubblewrap at either level. If you add plugins written by other people, full isolation keeps them away from
your config folder and music; at limits only, install only plugins whose code you have read or whose author
you trust.

### If it still says limits only

The words under the sandbox line say why:

| bubblewrap says | Why | What to do |
|---|---|---|
| `No permissions to create new namespace …` | The seccomp profile (in Docker), or the kernel does not allow user namespaces to ordinary users | `seccomp=unconfined`; outside Docker, see your distribution's switch (`kernel.unprivileged_userns_clone=1` on older Debian kernels) |
| `Failed to make / slave: Permission denied` | Docker's AppArmor profile | `apparmor=unconfined` |
| `setting up uid map: Permission denied`, or `loopback: Failed RTM_NEWADDR` with `Operation not permitted` | Ubuntu 23.10 and newer restrict user namespaces to programs AppArmor knows (`kernel.apparmor_restrict_unprivileged_userns = 1`), and the container is unconfined | On the host, `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` (and a file in `/etc/sysctl.d/` to keep it). This changes it for every program on the host; staying at limits only is the other choice |
| `Creating new namespace failed: nesting depth or /proc/sys/user/max_*_namespaces exceeded (ENOSPC)` | The host allows no user namespaces (`user.max_user_namespaces = 0`) | On the host, `sudo sysctl -w user.max_user_namespaces=15000` |
| `bubblewrap is not installed` | Encore runs outside the image without bubblewrap | Install it (`apt install bubblewrap`, `dnf install bubblewrap`) and restart Encore |

### /proc

Docker covers some files of the container's `/proc` (`/proc/kcore`, `/proc/timer_list`, …) and makes
`/proc/sys` read-only. The kernel then refuses a new `/proc` inside a user namespace, and bubblewrap says
`Can't mount proc on /newroot/proc: Operation not permitted`. Encore then runs plugins in full isolation
**without a `/proc`** (never with the host's or the container's): the log says
`plugins: full isolation (bubblewrap, no /proc)` and the card says Full isolation. Python plugins, the SDK and
the example plugin work without `/proc`. A plugin program that needs one (it reads `/proc/self/…`) gets it
with one more option, `systempaths=unconfined`, which shows the container's whole `/proc` and `/sys`
unmasked; add it only when a plugin's log says it needs `/proc`.

## Outside Docker

From the Linux archive, or a source build, Encore uses bubblewrap from `PATH`. Most distributions allow user
namespaces to ordinary users (Debian 11 and newer, Fedora, Arch); Ubuntu 23.10 and newer restrict them as in
the table above. The sandbox line tells you which level you got.
