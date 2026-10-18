# Porting a web/js module to web-rs (Rust/WebAssembly)

Goal: users see NO difference. The Rust module produces the same HTML (tags, ids, classes, data-* attributes, order,
text, placeholders, titles, aria), makes the same API calls, keeps the same localStorage keys and behaves the same.
The JS is the specification: read it line by line and port it 1:1. Do not "improve" anything; do not drop features;
do not change wording. Where the JS builds template strings, build the same string with `format!` (use `r##"…"##`
raw strings whenever the HTML contains `"#`, e.g. `href="#/library"`).

## Toolkit (read these files first: web-rs/src/dom.rs, json.rs, api.rs, ui.rs, app.rs, appearance.rs)
- JSON stays untyped like the JS: `serde_json::Value` + the `J` trait: `t.s("title")` (= `String(t.title ?? '')`),
  `t.i("id")`, `t.f("duration")`, `t.truthy("in_library")`, `t.a("tracks")` (Vec<Value>), `t.o("state")`, `t.st("x")`
  (Option<&str>, non-empty), `t.has("k")`, `t.is_nullish("k")`, `v.items()` (array → Vec<Value>), `json!({...})`.
  `typeof t.id === 'number'` → `t.g("id").is_number()`. `a.map(...).join('')` → `.iter().map(|x| ...).collect::<String>()`.
- API: `api::get(path).await`, `api::post(path, json!({..})).await`, `api::put`, `api::patch`, `api::del`,
  `api::post_empty(path)` (JS `api.post(p)` without body) → `Result<Value, ApiError>`; `e.message` is `e.message`.
  `try { … } catch (e) { toast(e.message, 'err') }` → `match … { Ok(v) => …, Err(e) => toast(&e.message, "err") }`.
- DOM: `dom::qs_in(&view, "#x")`, `dom::qsa_in`, `dom::set_html(&el, &html)`, `dom::value_of(&view, "[name=q]")`,
  `dom::checked_of`, `dom::data(&el, "id")` (= `el.dataset.id`), `dom::closest(e, "[data-i]")` (= `e.target.closest`),
  `dom::add_class/remove_class/toggle_class/has_class`, `dom::set_style`, `dom::hide`, `dom::focus`, `dom::prevent(e)`,
  `dom::key(e)`, `dom::set_disabled`, `dom::scroll_top()`, `dom::prompt/alert/confirm`, `dom::ls_get/ls_set/ls_remove`,
  `dom::is_phone()` (= matchMedia('(max-width: 760px)')), `dom::reload()`, `dom::set_hash`, `dom::dispatch("encore:x", None)`.
- Events: `dom::on(&el, "click", move |e| …)` (page-scoped, removed on the next navigation), `dom::on_sel(&view, "#save", "click", …)`,
  `dom::on_each(&view, ".tile", "click", |el, e| …)`, `dom::on_document("keydown", …)`. Listeners that must outlive navigations
  (the player bar, remote polling): `dom::on_forever`, `dom::on_window`. `e.target.closest(...)` delegation is the usual pattern.
  An `Element` is cheap to clone; clone what a closure needs (`let v = view.clone();`). Shared mutable state inside a page:
  `Rc<RefCell<…>>` (or `Rc<Cell<…>>`), cloned into closures.
- Async: `dom::spawn(async move { … })` for anything the JS did with `async` handlers; `dom::sleep(ms).await`;
  `dom::timeout(ms, || …)` (= setTimeout, page-scoped); `dom::interval(&view, ms, || …)` (= setInterval + ctx.onLeave);
  `ctx.on_leave(x)` keeps `x` (a `player::OnChange`, a `dom::EventListener`…) while the view is on screen.
- UI: `ui::esc`, `ui::icon("home")` (the icon set), `ui::icon_attr("check", r#"width="12" height="12""#)`,
  `ui::toast(msg, "ok"|"err"|"")`, `ui::modal(ModalOpts::new(title_html, body_html).footer(html).wide(true))` → `Modal { el, close() }`,
  `ui::confirm_dialog(title, text, "Delete", true).await -> bool`, `ui::fmt::{dur, long_dur, size, ago, date, num}`,
  `ui::art(src, label, cls)`, `ui::status_badge`, `ui::availability`, `ui::empty_state(icon, title, text)`, `ui::skeleton_grid(n)`,
  `ui::copy_text`, `ui::pop_menu(&anchor, vec![MenuItem::new("Label", &icon("x"), move || …), MenuItem::SEP, MenuItem::danger(…), MenuItem::link(…)])`
  (items the JS conditionally included (`cond ? {...} : null`) → push `Some(..)` only when `cond`), `ui::Debounce::new(ms, f).run()`,
  `ui::set_secret_text`, `ui::EYE/EYE_OFF`.
- App: `app::user()` (Option<Value>) / `ctx.user`, `app::info()`, `app::is_admin()`, `app::has_feature("remote")`,
  `app::navigate("/library")`, `app::avatar_html(&u, 32)`, `app::refresh_user().await`, `app::native_has("saveOffline")`,
  `app::native_call_with("saveOffline", Some(&json_string))`, `appearance::logo(30)`, `appearance::{ACCENTS, PRESETS, current, save, apply}`.
- Other modules: ONLY through the signatures in docs/archive/ui-rewrite/CONTRACT.md (player::play_queue, pages::common::album_tile,
  pages::playlists::import_modal, …). JS `await import('./x.js')` is just a direct call here.
- Pages: `pub fn render(view: Element, ctx: Ctx) -> PageFuture { Box::pin(async move { …; Ok(()) }) }`. `ctx.params`, `ctx.q("s")`
  (= ctx.query.s), `ctx.user`. Return `Err(msg)` where the JS would let an exception reach the router (it shows `.error-box`).
  Before writing into `view` after an await, the JS relied on detached views; you may do the same (`view` may be detached).
- `window.X = …` globals the JS exposed (encorePlayer, encoreRemote, encoreSession): set them with `js_sys::Reflect::set` on
  `dom::window()` using `wasm_bindgen::closure::Closure` (leaked with `.forget()`), so the desktop/Android apps keep working.
- Keep JS number formatting: `${n}` of a float like 2.5 prints "2.5", of 2.0 prints "2" (`crate::json::js_string`).
- `Math.random()` → `js_sys::Math::random()`. `Date.now()` → `dom::now_ms()`. `encodeURIComponent(s)` → `js_sys::encode_uri_component(s)`.
  `new URLSearchParams(obj)` → build the query string by hand (same key order). `confirm()`/`prompt()` → `dom::confirm/prompt`.

## Procedure
1. Read the JS file completely, the toolkit files, CONTRACT.md and the stub you replace (web-rs/src/…).
2. Write the Rust module. Keep the `//!` header (replace "TODO: port of" with "Port of"). Keep every `pub fn` of the contract with
   exactly those signatures; everything else private. Mirror the JS function names in snake_case.
3. `cd web-rs && cargo check --target wasm32-unknown-unknown` – fix every
   error AND warning that is in your file (errors in other stubs are not yours). If you truly need a web-sys feature that is
   missing, add it to Cargo.toml and mention it in your result.
4. Re-read the JS once more against your Rust: every string, every class, every branch. Note anything you could not port.
5. Copy the finished file to /tmp/claude-0/-home-user-test/a284e31c-e6e0-5e83-823f-66836ad7a8a5/scratchpad/ports/<same relative name, e.g. pages__library.rs>
   (that drop folder is read by the integrator; the file in your worktree is not).
