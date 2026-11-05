# Lantern

How Encore uses Lantern, the owner's own grabber (torrents, Usenet and Soulseek in one service), and what
waits for Lantern 1.0. Written for 0.42.20, before Lantern 1.0: its API is not final, so Encore does not
call it yet. The code is `server/crates/requests/src/lantern/`, the routes are
`server/crates/api/src/routes/lantern.rs`, the card is `web-rs/src/settings/lantern.rs`.

## Settings

| key | env | what |
| --- | --- | --- |
| `lantern_url` | `LANTERN_URL` | Lantern's base address (trailing slashes trimmed) |
| `lantern_api_key` | `LANTERN_API_KEY` | its API key (secret: never sent to a browser) |
| `lantern_took_over` | — | set by the move, cleared by Disconnect |

**Connected** means the address and the key are both set. **Taken over** means connected and
`lantern_took_over`. Only "taken over" changes anything:

- the request worker's sources `slskd_lossless`, `torznab` and `slskd` become one `lantern` turn, at the
  place of the first of them (`lantern::route`);
- Encore's Soulseek engine stops (the account lives in Lantern);
- GET /api/plugins and GET /api/plugins/{id} mark the cards `torznab`, `slskd` and every download client
  `handled_by: "lantern"`. The Plugins page shows a note instead of their settings.

Disconnecting clears all three settings. Encore's own settings were never removed, so they come back as they
were.

## Routes (admins)

- `GET /api/admin/lantern`: `{url, key_set, env_locked, connected, took_over, api_ready, waiting}`.
- `PUT /api/admin/lantern` `{url, api_key?}`: save. A missing or masked key keeps the saved one. 400 for an
  address that isn't http(s) with a host, 409 while environment variables set them.
- `DELETE /api/admin/lantern`: disconnect (409 while environment variables set them).
- `POST /api/admin/lantern/migrate` `{dry_run}` (`dry_run` defaults to true):
  - dry run: `{dry_run: true, plan: {items: [{part, key, label, value, secret, default}], skipped: [..]}}`.
    `part` is "Indexers (Prowlarr)", "Torrent client", "Usenet client" or "Soulseek". A secret's `value`
    is the mask when set, "" when not. A card that is switched off lands in `skipped` with the reason.
  - the move: 409 without a connection, **501 until Lantern 1.0** (nothing changes).
- `POST /api/system/test/lantern` (the kit's test route): `{ok, message}`. Checks that Lantern answers over
  HTTP; the key is checked once the API is final.

## Waiting for Lantern 1.0

These need Lantern's final API and are not built. Each place answers `lantern::NOT_YET` until they are:

1. **Search and grab.** The worker's `lantern` turn (`worker/pick.rs`, `next_source`) passes to the next
   source today. It should search albums and tracks through Lantern (all its backends), rank the results
   the way the Soulseek and indexer turns do, and grab the best one.
2. **Live progress.** Follow Lantern's event stream for a grab and write its progress into the request,
   the way `poll_slskd` / `poll_torznab` do, with stalls handled by the worker's existing timeouts.
3. **Import.** Find a finished grab's files (a path shared with Encore, or a download from Lantern) and
   hand them to the existing import step, which tags and files them.
4. **The move.** Write each item of `migrate::plan` into Lantern's settings (indexers, torrent and Usenet
   clients, the Soulseek account), then set `lantern_took_over` (`migrate::apply`).
5. **The key check** in the connection test.
