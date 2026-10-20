//! Encore's web app, in Rust (WebAssembly). A 1:1 port of web/js: every module here mirrors the JavaScript
//! file of the same name – same HTML, same CSS classes, same text, same API calls, same localStorage keys –
//! so users see no difference. Pages build HTML strings and wire events exactly like the JS did; dom.rs,
//! json.rs, api.rs and ui.rs are the small toolkit that makes that natural in Rust.
// `!(x > 0.0)` is on purpose where it appears: it also catches NaN, which `x <= 0.0` lets through
#![allow(clippy::neg_cmp_op_on_partial_ord)]
#![allow(clippy::type_complexity)]

pub mod api;
pub mod app;
pub mod appearance;
pub mod details;
pub mod dom;
pub mod group;
pub mod json;
pub mod num;
pub mod offline;
pub mod pages;
pub mod playback;
pub mod player;
pub mod remote;
pub mod scrubber;
pub mod settings;
pub mod sw;
pub mod ui;

#[cfg(test)]
mod parity;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(on_panic));
    ui::init();
    details::init();
    appearance::init();
    app::init();
}

/// A Rust panic in the page used to leave it half drawn with no word of what happened (0.26.0/0.26.1: empty
/// Library and Requests pages). Now it is printed to the console (as before), a banner says it, and the report
/// goes to the server (Settings → Diagnostics → App crashes, kind "web"). Only plain JS runs here: the Rust
/// side may hold borrows that a second call into the module would trip over.
fn on_panic(info: &std::panic::PanicHookInfo<'_>) {
    console_error_panic_hook::hook(info);
    let stack = js_sys::Reflect::get(&js_sys::Error::new(""), &JsValue::from_str("stack")).ok().and_then(|v| v.as_string()).unwrap_or_default();
    let text = format!("{info}\n\nStack:\n{stack}");
    let f = js_sys::Function::new_with_args("text", PANIC_JS);
    let _ = f.call1(&JsValue::NULL, &JsValue::from_str(&text));
}

const PANIC_JS: &str = r#"
try {
  var first = (text.split('
').filter(function (l) { return l && l.indexOf('panicked at') < 0 && l.indexOf('Stack:') < 0; })[0] || text).slice(0, 200);
  var where = (text.match(/panicked at ([^
]*)/) || [])[1] || '';
  if (!document.getElementById('panic-banner')) {
    var b = document.createElement('div');
    b.id = 'panic-banner';
    b.setAttribute('role', 'alert');
    b.style.cssText = 'position:fixed;left:0;right:0;top:0;z-index:99999;padding:10px 14px;background:#7a1f1f;color:#fff;font:14px/1.4 system-ui,sans-serif;display:flex;gap:12px;align-items:center;flex-wrap:wrap';
    var t = document.createElement('span');
    t.textContent = 'This page hit an error and stopped: ' + first + (where ? ' (' + where.replace(/:$/, '') + ')' : '') + '. The report was sent to Settings → Diagnostics.';
    var r = document.createElement('button');
    r.textContent = 'Reload';
    r.style.cssText = 'padding:6px 12px;border:0;border-radius:8px;background:#fff;color:#7a1f1f;font:inherit;cursor:pointer';
    r.onclick = function () { location.reload(); };
    b.appendChild(t); b.appendChild(r);
    (document.body || document.documentElement).appendChild(b);
  }
  fetch('/api/diagnostics/client-crash', {
    method: 'POST', credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', 'X-Requested-With': 'encore' },
    body: JSON.stringify({ kind: 'web', device: navigator.userAgent.slice(0, 200), os: navigator.platform || '',
      summary: first + (where ? ' @ ' + where.replace(/:$/, '') : ''), text: text + '

page: ' + location.href,
      key: first + '|' + where + '|' + location.hash })
  }).catch(function () {});
} catch (e) {}
"#;
