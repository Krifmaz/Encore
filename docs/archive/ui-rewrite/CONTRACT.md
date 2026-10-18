# web-rs module contract

Rust signatures every module exposes. Ports call each other ONLY through these (they mirror the JS exports).
Toolkit (already written, read them): `src/dom.rs`, `src/json.rs` (trait `J`: `v.s("k")`, `v.i`, `v.f`, `v.truthy`, `v.a`, `v.o`,
`v.items()`, `js_string`), `src/api.rs` (`api::get/post/put/patch/del(path[, body]) -> Result<Value, ApiError>`), `src/ui.rs`
(`esc`, `icon("name")`, `icon_attr`, `toast`, `modal(ModalOpts)`, `confirm_dialog`, `fmt::{dur,long_dur,size,ago,date,num}`, `initials`,
`art`, `status_badge`, `availability`, `Debounce`, `empty_state`, `skeleton_grid`, `copy_text`, `pop_menu(anchor, Vec<Option<MenuItem>>)`,
`set_secret_text`, `EYE`, `EYE_OFF`), `src/app.rs` (`app::user()`, `app::info()`, `app::is_admin()`, `app::has_feature`, `app::navigate`,
`app::avatar_html`, `app::refresh_user()`, `app::native_has/native_call`, `Ctx`, `PageFuture`), `src/appearance.rs`.

Conventions: `Value` = `serde_json::Value`; track/album/request objects stay `Value` (read fields with the `J` trait, like the JS did).
Callbacks are `Rc<dyn Fn()>` / `Rc<dyn Fn(Value)>`. Functions the JS declared `async` but callers never awaited are plain `fn` that
`dom::spawn` internally. Page renderers return `PageFuture` (`Box::pin(async move { … Ok(()) })`; `Err(String)` shows the error box).

```rust
// src/player.rs  (web/js/player.js)
pub struct Link { pub id: String, pub name: String, pub state: Value, pub online: bool, pub send: Rc<dyn Fn(Value)> }
pub struct OnChange(/* dropping it unsubscribes */);
pub fn shuffle_library() -> impl Future<Output = usize>;      // async; 0 ⇒ caller toasts "Your library is empty"
pub fn on_change(f: impl Fn(Option<Value>) + 'static) -> OnChange;
pub fn current() -> Option<Value>;
pub fn linked() -> Option<Link>;                // Link is Clone
pub fn set_link(l: Option<Link>);
pub fn link_state(s: Value, online: bool);
pub fn snapshot() -> Value;
pub fn set_volume(v: f64);
pub fn command(c: Value);
pub fn queue() -> (Vec<Value>, usize);
pub fn play_queue(tracks: Vec<Value>, start: usize, context: &str);   // context: "list" | "album" | "single" | "library" | "preview"
pub fn enqueue(tracks: Vec<Value>);
pub fn play_next(tracks: Vec<Value>);
pub fn preview(t: Value);
pub fn can_save_offline() -> bool;
pub fn save_offline(tracks: Vec<Value>);
pub fn seek_to(sec: f64);
pub fn toggle();
pub fn next(auto: bool);                        // spawns the async work
pub fn prev();
pub fn mount();
pub fn set_full(on: bool);

// src/remote.rs  (web/js/remote.js)
pub fn link_to(d: Value);
pub fn unlink();
pub fn start();                                  // reads app::info() for the "remote" feature
pub fn open();                                   // the Devices modal (also window.encoreRemote.open)

// src/pages/common.rs
pub async fn submit_request(body: Value, btn: Option<Element>) -> Option<Value>;
pub fn album_tile(a: &Value, href: &str, actions: &str, sub: Option<&str>) -> String;
pub fn library_tile(a: &Value) -> String;
pub fn bind_tiles(root: &Element);                                    // navigates with app::navigate
pub fn request_row(r: &Value, admin: bool) -> String;
pub fn bind_request_actions(root: &Element, reload: Rc<dyn Fn()>);
pub fn clear_menu(anchor: &Element, reload: Rc<dyn Fn()>, mine: bool);

// src/pages/picker.rs
pub fn picker_modal(opts: Value);              // opts: {kind, artist, title, album, mbid, artist_mbid, track_count, length, year, cover_url, isrc, request_id}

// src/pages/library.rs
pub fn play_album(id: &str, start: usize);     // spawns
pub fn track_menu(anchor: &Element, t: &Value, admin: bool, on_deleted: Option<Rc<dyn Fn()>>);
pub fn fix_song_modal(t: &Value);
pub fn render(view: Element, ctx: Ctx) -> PageFuture;
pub fn render_album(view: Element, ctx: Ctx) -> PageFuture;

// src/pages/discover.rs
pub fn bind_request_buttons(root: &Element);
pub fn render_search / render_artist / render_artist_by_name / render_album (view: Element, ctx: Ctx) -> PageFuture;

// src/pages/foryou.rs
pub fn more_like_this(artist: &str, title: &str, mbid: &str);        // spawns; mbid "" = none
pub fn artist_card(a: &Value) -> String;
pub fn render / render_tag (view: Element, ctx: Ctx) -> PageFuture;

// src/pages/playlists.rs
pub fn import_modal(url: &str, into: Option<i64>);
pub fn album_choice(title: &str, go: Rc<dyn Fn(Option<String>)>);    // "song" | "album" | None (admin default)
pub fn add_to_playlist_modal(track_ids: Vec<i64>);                    // spawns
pub fn render / render_playlist (view: Element, ctx: Ctx) -> PageFuture;

// src/pages/artwork.rs
pub fn artwork_modal(album_id: &str, a: &Value, on_done: Rc<dyn Fn()>);
// src/pages/lyrics.rs
pub fn lyrics_modal(t: &Value);                 // spawns
pub fn fetch_album_lyrics(album_id: &str);      // spawns

// src/pages/settings.rs
pub fn render(view: Element, ctx: Ctx) -> PageFuture;
pub async fn about_card() -> String;
// sub-pages settings.js renders into its panel – same PageFuture shape:
// src/pages/account.rs, admin.rs (render_users, render_invites), connections.rs, autodownload.rs, diagnostics.rs
pub fn render(view: Element, ctx: Ctx) -> PageFuture;

// src/pages/activity.rs, requests.rs, download.rs, home.rs, leaderboard.rs
pub fn render(view: Element, ctx: Ctx) -> PageFuture;
// src/pages/storage.rs
pub fn render(view: Element, ctx: Ctx) -> PageFuture;
pub fn sidebar_meter(el: Element);              // spawns
// src/pages/auth.rs
pub async fn render(root: Element, path: String, query: HashMap<String, String>);
```
