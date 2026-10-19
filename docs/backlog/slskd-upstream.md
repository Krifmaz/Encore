# slskd: what Encore would like upstream

Encore runs slskd (0.26.1, the newest release in October 2026) as its Soulseek engine and talks to it over
its HTTP API. Since 0.29 Encore keeps slskd's own reconnect loop switched off and reconnects on its own
schedule (`server/crates/requests/src/slskd_session/`). Three things in slskd make that harder than it needs
to be. This is the text for an issue or pull request at github.com/slskd/slskd.

## 1. The login wait can't be set (the "5000 milliseconds")

`src/slskd/Program.cs` builds the client as

```csharp
new SoulseekClient(minorVersion: NetworkMinorVersion, options: new SoulseekClientOptions(
    maximumConcurrentUploads: …, maximumConcurrentDownloads: …, minimumDiagnosticLevel: …,
    maximumConcurrentSearches: 2, raiseEventsAsynchronously: true))
```

so Soulseek.NET's `messageTimeout` keeps its default of 5000 ms, and `SoulseekClientOptionsPatch` (in
`Application.cs`) has no field for it either. The login waits for the server's answer with that timeout:
when the server is slow to answer (for example while it rate-limits an address), every attempt fails with
"The wait timed out after 5000 milliseconds" although the connection itself works.
`soulseek.connection.timeout.connect` (default 10000) only covers the TCP connect.

Asked for: an option `soulseek.connection.timeout.message` (milliseconds, default 5000, env
`SLSK_MESSAGE_TIMEOUT`) passed to `SoulseekClientOptions(messageTimeout: …)`. Encore would set it to 30000.

## 2. The reconnect schedule can't be changed

`src/slskd/Core/ConnectionWatchdog.cs` retries with
`Compute.ExponentialBackoffDelay(attempts, baseDelayInMilliseconds: 1000, maxDelayInMilliseconds: 300000)`,
which gives 0–1 s, 1 s, 3 s, 7 s, 15 s … after the first failures, and `Application.Client_Disconnected`
starts the watchdog again (attempt count 0, so at once) after every disconnect except "by the user", a
rejected login, another client logging in and the VPN going down. A login that times out therefore repeats
every few seconds. A Soulseek server that ignores logins from an address that knocks this often never
lets it in again.

Asked for, any of:
- options for the base and the maximum delay (`soulseek.connection.reconnect.delay_min` /
  `delay_max`), with a first delay of at least 30 s by default;
- an option to switch the automatic reconnect off (`flags.no_reconnect`), for clients that reconnect
  themselves with `PUT /api/v0/server`;
- `PUT /api/v0/server?once=true`: one attempt without starting the watchdog's retry loop.

What 0.26.1 does, seen against a fake Soulseek server (Oct 19): after a session drops, the watchdog's next
attempt starts within 50 ms; `DELETE /api/v0/server` while the client is still connecting (TCP) cancels the
attempt with "Disconnected from the Soulseek server: Operation cancelled", which `Client_Disconnected`
treats as an ordinary disconnect, so the watchdog starts again at once; a DELETE once the client is
connected (logging in or logged in) ends it "by the user" and the watchdog stays off; a DELETE while the
watchdog waits between two attempts stops it without a word. So a client can't keep the loop off with the
API alone.

Until then Encore keeps slskd away from the server instead: slskd.yml names a relay of Encore's on
127.0.0.1 as the Soulseek server (through the VPN, Encore's SOCKS proxy plays the same part), and Encore
lets one connection through per attempt of its own (`PUT /api/v0/server`); slskd's own attempts are reset
on loopback. Encore quiets the loop with `DELETE /api/v0/server` once slskd waits between two attempts
("Waiting about … before attempting to reconnect"). See `server/crates/requests/src/slskd_session/door.rs`.

Also asked for: `DELETE /api/v0/searches/{id}` only forgets a search; one that still runs goes on until its
timeout (only `PUT /api/v0/searches/{id}` stops it). Stopping it first would match what the name suggests.

## 3. Why the session ended is only in the log

`GET /api/v0/server` and the application state say `Disconnected`, not why. Asked for: a
`server.lastDisconnect { at, reason, message }` in the state (reason: `user`, `shutdown`, `loginRejected`,
`kicked`, `vpn`, `error`; message: the exception's message, e.g. "The server rejected login attempt:
INVALIDPASS"), so clients don't have to read the log to tell a rejected password from a timeout.
