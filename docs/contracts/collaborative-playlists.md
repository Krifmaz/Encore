# Collaborative playlists (0.37)

The owner of a playlist turns **Collaborative** on and invites other accounts on the same server, by user name or
with an invite link. Whoever joins sees the playlist among their own playlists and can **add, remove and reorder
its songs** (and run Remove duplicates / Re-check library). Only the owner renames, shares, syncs, follows,
downloads missing songs for, or deletes it.

Builds on the existing playlist model: it is still one `playlists` row of the owner's (no copies), so every change
shows for everyone at once. Separate from share links (`/s/<token>`, read-only, anyone with the link).

## Schema 34

- `playlists.collaborative INTEGER NOT NULL DEFAULT 0`
- `playlists.invite_token TEXT` (unique; NULL = no invite link)
- `playlist_collaborators (playlist_id, user_id, status 'invited'|'joined', invited_by, created_at, joined_at)`;
  the owner is never in it.

Only kind `playlist` can be collaborative (not Liked Songs, not someone else's playlist added from a share link).

## Rules

- **Off** pauses it: members and invites stay but see nothing, and the invite link stops working, until it is on
  again. Inviting someone or making an invite link turns it on.
- A disabled owner ends it for members (as with share links). Deleting the playlist deletes its members and invites;
  deleting an account deletes that account's memberships.
- A member's `DELETE /api/playlists/{id}` is **Leave** (the playlist stays the owner's).
- Members never see the owner's share link, sync targets, source ids or invite link.

## API

| Route | Who | |
|---|---|---|
| `PATCH /api/playlists/{id} {collaborative: bool}` | owner | the toggle |
| `GET /api/playlists/{id}/collaborators` | owner, member | `{name, collaborative, role, owner, members: [{user_id, username, status}], invite_link}` — a member sees only who joined, and `invite_link: null` |
| `POST /api/playlists/{id}/collaborators {username}` | owner | invite (case-insensitive, `@` allowed); 404 unknown account, 400 yourself → the members view |
| `DELETE /api/playlists/{id}/collaborators/{uid}` | owner (anyone), user (themselves) | remove / take back an invite / leave |
| `POST /api/playlists/{id}/invite-link` | owner | `{token, path: "/#/collab/<token>", url}` (same link when asked again) |
| `DELETE /api/playlists/{id}/invite-link` | owner | the link stops working; a new one is a new token |
| `GET /api/collab/invites` | anyone | `[{playlist_id, name, owner, count}]` waiting for the user |
| `POST /api/collab/invites/{id}/accept` / `decline` | invited user | join / say no |
| `POST /api/collab/join {link}` | anyone | join with an invite link (any text with `collab/<token>`, or a bare token) → `{id, name, owner}` |
| `POST /api/collab/join/check {link}` | anyone | `{id, name, owner, count, own, joined}` without joining |

Song routes that accept members: `GET /api/playlists/{id}`, `POST …/tracks`, `DELETE …/items/{iid}`, `POST …/move`,
`POST …/dedupe`, `POST …/rematch`, `POST /api/playlists/import {into}`.

`GET /api/playlists` lists joined playlists among the user's own (by `updated_at`). Every own or joined playlist in
the list and detail carries `collab`: `null` (never collaborative) or `{role: "owner"|"collaborator", owner, members}`.

## Clients

- **Web** (`web-rs/src/pages/collab.rs`): Collaborative switch + Invite window on the playlist page; a member's
  badge, Members and Leave; invites (Join / Decline) on top of Playlists; `#/collab/<token>` asks "Join?"; an invite
  link pasted into Add a shared playlist joins.
- **Desktop** (`clients/desktop/src/pages/playlist.rs`, `playlists.rs`): the same.
- **Android** (`features/library/PlaylistCollabRules.kt`, `PlaylistCollabUi.kt`): the playlist ⋯ has Make
  collaborative / Invite collaborators / Turn off collaborative, or a member's Members / Leave; invites on top of
  Playlists; an invite link pasted into Add a shared playlist joins.
