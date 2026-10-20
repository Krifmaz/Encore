# Encore desktop (clients/desktop) – how the crate is built

Read this before writing a page. The crate is an Iced 0.14 daemon (`src/main.rs` → `src/app.rs`);
pages are modules under `src/pages/`, widgets under `src/widgets/`, and the only audio path is the
embedded Cadenza engine (`src/engine/`). Rules for everyone: plain words, no exclamation marks,
`cargo check` while iterating, `cargo clippy -- -D warnings` clean before you report.

## Layout

| Path | What it is | Owner |
|---|---|---|
| `src/main.rs` | flags (`--server`, `--minimized`, `--screenshot-tour <dir> [--login u:p]`), logging, single instance, `iced::daemon` | architect / integrator |
| `src/app.rs` | `App`, `Message` (one variant per module), `update` routing, the shell (`view`), `subscription` | architect / integrator |
| `src/api.rs` | `Api` (reqwest + cookies + `X-Requested-With: encore`), `ApiError`, `Session` | architect |
| `src/config.rs` | `config.json` (servers, appearance, bar buttons, hotkeys, window…), `BAR_ACTIONS` | architect |
| `src/theme.rs` | the web's tokens as `Palette`, `theme::look()`, every widget style (`theme::btn(Btn::Primary)`, `theme::input`, `theme::card`, …) | architect |
| `src/widgets/` | the toolbox (below). Stage 2: add `pub mod yours;` to `widgets/mod.rs` and a `widgets/yours.rs` | architect; pages add files |
| `src/pages/mod.rs` | `Page`, `Ctx`, `Effect`, the page module contract | architect |
| `src/pages/<page>.rs` | one page each (stubs in stage 1) | packages A–E |
| `src/pages/settings/` | `Section` (nav ids and labels) + the settings pages | package E |
| `src/player/` | `Snapshot`, `State`, the bar, Customize the bar | package F |
| `src/engine/` | Cadenza: `start`, `play`, `send`, `subscription`, `tracks::from_encore` | package F |
| `src/link.rs` | the device link (app token + `cadenza::encore::start`) | package F |
| `src/desktop/` | the channel from outside the window (`desktop::send`), single instance; tray, media keys, hotkeys, mini player, updater, offline store | package G |
| `src/tour.rs` | the screenshot tour | architect |
| `src/fmt.rs` | `size`, `mmss`, `ago`, `date`, `quality`, JSON field helpers `s/f/i/truthy/items` | architect |

## The page module contract

Every page module (see any stub, e.g. `src/pages/requests.rs`) has exactly these:

```rust
#[derive(Debug, Default)] pub struct State { … }
#[derive(Clone, Debug)]  pub enum Message { … }

/// Called when the app navigates here, with the route's parameters (see the table).
pub fn open(state: &mut State, <params>, ctx: &mut Ctx) -> Task<Message>
pub fn update(state: &mut State, msg: Message, ctx: &mut Ctx) -> Task<Message>
pub fn view<'a>(state: &'a State, ctx: &Ctx<'a>) -> El<'a, Message>
/// A modal or ⋯ menu laid over the whole window, when one is open (None otherwise).
pub fn overlay<'a>(state: &'a State, ctx: &Ctx<'a>) -> Option<El<'a, Message>>
pub fn subscription(state: &State) -> Subscription<Message>
```

`open` parameters per page (from `Page` in `pages/mod.rs`):

| page module | `open` params | `Page` variant |
|---|---|---|
| home, for_you, requests, playlists, download, soulseek, leaderboard, storage, bug_reports, offline, login | none | the matching variant |
| search | `q: String` | `Page::Search { q }` |
| tag | `name: String` | `Page::Tag(name)` |
| artist | `id: String` (a MusicBrainz id, or a name for `Page::ArtistName`) | `Page::Artist(id)` / `Page::ArtistName(name)` |
| album | `id: String` | `Page::Album(id)` |
| library | `tab: String` ("" = albums) | `Page::Library { tab }` |
| library_album | `id: String` | `Page::LibraryAlbum(id)` |
| library_artist | `name: String` | `Page::LibraryArtist(name)` |
| playlist | `id: i64` | `Page::Playlist(id)` |
| bug_report | `id: i64` | `Page::BugReport(id)` |
| settings | `section: Section` | `Page::Settings(section)` |

`El<'a, M>` is `iced::Element<'a, M>` (default theme and renderer). Return `Task::none()` when
there is nothing to do. `Message` must be `Clone + Debug`; put `serde_json::Value` in it freely.
Keep the page's own `Message` small: box large payloads (`Box<Value>` for whole album answers).

The app (`app.rs`) already has, for every page: the `Message::<Page>(pages::<page>::Message)`
variant, the `App.<page>` state field, the `open` call in `navigate`, the `update` routing
(`page_update!`), the `view` / `overlay` / `subscription` dispatch. You do not touch `app.rs`.

### How a page is built, step by step

1. State: what the page shows (`Option<Vec<Value>>` for lists: `None` = loading, `Some(empty)` =
   nothing) plus inputs, open dialog, open menu, selection.
2. `open`: reset what must reload, start the requests:
   ```rust
   let api = ctx.api.clone();
   Task::perform(async move { api.get("/library/recent?limit=12").await }, Message::Recent)
   ```
   `Api` returns `Result<serde_json::Value, ApiError>`; read fields with `fmt::s(&v, "title")`,
   `fmt::f`, `fmt::i`, `fmt::truthy`, `fmt::items(&v)` (a list, or `{items: [...]}`).
3. `update`: store answers, start more tasks, and ask the app for things through `ctx` (below).
4. `view`: compose widgets. Titles with `widgets::h1/h2/h3`, lines with `widgets::muted/small`,
   buttons with `widgets::btn(label, Btn::Primary, Some(Message::X))` (`None` disables), lists with
   `widgets::TrackRow`, grids with `widgets::Tile` + `widgets::grid`, sections with
   `widgets::section_head`, loading with `widgets::skeleton_rows/skeleton_tiles`, nothing with
   `widgets::empty`. Text in any script: use `widgets::t(...)` instead of `iced::widget::text`.
5. `overlay`: when `state.dialog.is_some()` return `widgets::modal(title, body, footer, wide,
   Message::CloseDialog)`; when `state.menu.is_some()` return `widgets::pop_menu(at, items,
   Message::CloseMenu, ctx.size)`.

The behaviour reference for each page is the web page named in the stub's first line
(web-rs/src/pages/*.rs); the words are docs/ui/strings-reference.txt; the API calls are the ones
the web makes (same paths, same bodies).

## Ctx: what a page sees and what it can ask for

`Ctx` is built by the app for every `open`, `update` and `view` call.

Reads:
- `ctx.api: Arc<Api>` – the server. Clone it into async tasks. `ctx.base()` is "http://host:port".
- `ctx.config: &Config` – the saved settings (`ctx.config.bar_buttons()`, `.appearance`, `.hotkeys`;
  `ctx.config.local("encore.libSort")` for what the web keeps in localStorage, under the same keys).
- `ctx.session: &Session` – `username()`, `user_id()`, `is_admin()`, `is_owner()`, `perm("pick")`,
  `has_feature("soulseek")`, `app_name()`, the raw `user` and `info` JSON. Shortcuts: `ctx.is_admin()`,
  `ctx.has_feature(..)`, `ctx.username()`.
- `ctx.player: player::Snapshot` – `status` (the engine's `Status`: `track`, `state`, `position`,
  `duration`, `queue`, `index`, `volume_linear`, `error`), `track()`, `current_id()`.
- `ctx.cover(album_id, &row["cover_v"], 400)` → `Option<image::Handle>`: the album's cover at that
  size (400 tiles, 80 rows, 600 the player). `None` while loading; the app fetches it and the next
  frame shows it. `ctx.picture(url_or_path)` for any other image.
- `ctx.look: Palette` – the colours (`ctx.look.accent`, `.muted`, `.err`…). `ctx.size`, `ctx.wide(1100.0)`,
  `ctx.cursor` (the pointer, where a right-click menu opens), `ctx.previous` (the page before this
  one; a bug report sends its `hash()` along).

Asks (collected during `update`, applied by the app right after):
- `ctx.toast("…")`, `ctx.toast_ok("…")`, `ctx.toast_warn("…")`, `ctx.toast_err(e.message)`.
- `ctx.navigate(Page::LibraryAlbum(id))`, `ctx.replace(page)` (no history entry). The mouse's back
  button goes back through the history.
- `ctx.play(tracks, index)` – Encore track rows (JSON as the server sends them) through the engine;
  `ctx.play_next(tracks)`, `ctx.enqueue(tracks)`.
- `ctx.copy(text)` (pair it with a toast), `ctx.open_url(url)` (the browser).
- `ctx.copy_text(text)` – the web's `copy_text`: copies, reads the clipboard back and toasts "Copied to
  clipboard" or "Copy failed – select and copy manually". `ctx.details(title, text)` opens the details
  dialog over everything: the text whole, wrapped and selectable (a read-only editor), with Close and
  Copy. `widgets::details::copy_btn(msg)` is the small Copy button an error line carries. Use them for
  any error or status a line cuts short or that someone may paste into a bug report.
  `ctx.details_with(title, text, lead, act)` adds a warning box above the text (`details::Lead`: why,
  in full, and where from) and one button beside Copy (`details::Act`: label, the path to POST with its
  body, the toast once it worked); the app sends it, closes the dialog and reloads the open page's request
  list. A request's Keep anyway / Download anyway uses it.
- `ctx.edit_config(|c| c.appearance.theme = "oled".into())` – saved, look re-applied.
- `ctx.player_msg(player::Message::…)` – e.g. open the queue or Customize the bar.
- `ctx.signed_in(user)` (login only), `ctx.sign_out()`, `ctx.refresh_session()` (after a profile change),
  `ctx.refresh_shell()` (the sidebar's counts, e.g. the bug badge), `ctx.forget_cover(url)` (after the
  artwork changed).
- `crate::desktop::offline::save(ctx, rows)` – "Save for offline" for library rows (it toasts itself).
- `api.patch(path, body)` next to `get`, `post`, `put`, `del`, `post_form`.

Asks do nothing in `view`; `view` only reads.

## Adding a Message

To your page's `Message`: add the variant, handle it in `update`. Nothing else: the app routes
`Message::<Page>(m)` to your `update` and maps your tasks back.

To the top-level `Message` (app.rs): only the architect (stage 1) and the integrator (stage 3).
Pages never need it; player, link, desktop and tour have their own variants already.

## Modals, menus, confirms

- A modal: hold `dialog: Option<Dialog>` in your state, draw it in `overlay` with
  `widgets::modal(title, body, Some(footer_row), wide, Message::CloseDialog)`. The X, the backdrop
  and the footer's Cancel send `CloseDialog`. A yes/no: `widgets::confirm(title, text, "Delete",
  danger, Message::DoIt, Message::CloseDialog)`.
- The ⋯ menu: hold `menu: Option<(Point, MenuTarget)>`; the ⋯ button and `TrackRow::on_menu`
  (also fired by a right-click) send `Message::Menu(target)`; in `update` store
  `(ctx.cursor, target)`; in `overlay` return `widgets::pop_menu(at, vec![MenuItem::new("Play next",
  Message::PlayNext(id)).icon(Icon::Plus), MenuItem::new("Delete", ..).danger()], Message::CloseMenu,
  ctx.size)`.
- Toasts come from `ctx.toast*`; the app draws the stack bottom right for 4 s. An error toast stays
  until it is clicked away and carries Copy (at most three stay; the same error is not shown twice).

## Navigating

`ctx.navigate(Page::…)`. `Page` is the hash router of the web app (`Page::from_hash("#/library/album/x")`
parses the links the server sends; `page.hash()` makes one). The sidebar lights the entry of
`page.nav_key()`. The app scrolls the main area to the top on every navigation.

## Theme and widgets

`theme::look()` gives the active `Palette` anywhere (styles read it; views can too, or use `ctx.look`).
Button kinds: `Btn::Primary | Default | Ghost | Danger | Icon | Nav | NavOn | Tab | TabOn | Seg | SegOn | Row | Play`.
Badges: `widgets::badge("FLAC", Badge::Plain | Ok | Warn | Err | Info | Accent)`.
Containers: `theme::card`, `theme::panel2`, `theme::art`, `theme::accent_box`, `theme::error_box`.
Inputs: `text_input(..).style(theme::input)`, `toggler(..).style(theme::switch)`,
`pick_list(..).style(theme::pick).menu_style(theme::pick_menu)`, `slider(..).style(theme::slider_style)`,
`progress_bar(..).style(theme::progress)`, `scrollable(..).style(theme::scroll)`.
Icons: `widgets::icon(Icon::Heart, 16.0, color)`; the names are the web's (`Icon::by_name("disc")`).
Padding with four sides: `widgets::pad(top, right, bottom, left)` (iced arrays take two).

## The engine and the player

`engine::start()` runs Cadenza from `<config dir>/encore-desktop/cadenza/config.toml`; without a sound
device it uses the null output. `engine::play(tracks, index, 0.0)`, `engine::send(Command::Toggle)`,
`engine::status()`, `engine::subscription()` (status, track started, played enough). Server tracks
become `engine::tracks::from_encore(base, &auth, &row)` with `Authorization: Bearer <app token>` (or the
session cookie before a token exists) and `X-Encore-Decoders`. Pages do not call the engine: they
`ctx.play(...)` and the app does. The player's bar reads `player::State.status` from the events.

The link (`link.rs`) asks for an app token once per server, stores it in the config, and starts
`cadenza::encore::start` with `device_id = "desktop-<fnv1a(hostname|server)>"`, `embedded: false`, so
the computer is listed in Your devices and commands from other devices drive the same engine.

## Checks

`cargo check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and the tour:

```
xvfb-run -a -s "-screen 0 1400x900x24" ./target/debug/encore-desktop \
  --server http://127.0.0.1:18085 --login admin:adminpass1234 --screenshot-tour /tmp/shots
```

The tour uses a temporary config dir, signs in, saves `login.png`, `home.png`, `playing.png`, one
PNG per page and `player-customize.png`, then exits 0. `RUST_LOG=info` shows each step.
