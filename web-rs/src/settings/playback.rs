//! Port of web/js/pages/settings.js (renderPlayback) – see docs/archive/ui-rewrite/CONTRACT.md
//! Settings → Playback: quality, buffering, the end of the queue, sound effects, song transitions, server
//! processing (Cadenza on the server) and the engine of a linked Cadenza device.

use crate::api;
use crate::app::{self, Ctx, PageFuture};
use crate::dom;
use crate::json::{js_string, json, J};
use crate::ui::{esc, escv, icon, toast};
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use web_sys::Element;

/// The state of the JS `renderPlayback` closure: the panel, `p`, `serverPrefs` and the server's Cadenza answer.
struct Look {
    el: Element, p: RefCell<Value>, server_prefs: bool, dsp: Value,
    /// what the Cadenza engine section was last painted from (`engine_key`)
    painted: RefCell<String>,
}

/// Settings → Playback: stream quality, buffering, what happens when the queue ends, server-side Cadenza processing of streams
/// (where an admin switched it on and allowed the account; greyed out otherwise) and the switches of a linked Cadenza device.
pub fn render(el: Element, _ctx: Ctx) -> PageFuture {
    Box::pin(async move {
        let server_prefs = app::has_feature("theme");
        let p = crate::playback::current();
        // server-side Cadenza: on for this server, and allowed for this account? (fetched once per visit to the tab)
        let dsp = api::get("/playback/dsp").await.unwrap_or_else(|_| json!({ "enabled": false }));
        let st = Rc::new(Look { el, p: RefCell::new(p), server_prefs, dsp, painted: RefCell::new(String::new()) });
        // what the Cadenza engine section is painted from (the linked device's engine) – a repaint only when it changes
        let st2 = st.clone();
        dom::interval(&st.el, 2000, move || {
            if st2.el.is_connected() && engine_key() != *st2.painted.borrow() { paint_playback(&st2); }
        });
        paint_playback(&st);
        Ok(())
    })
}

const QUALITY_HELP: [(&str, &str); 3] = [
    ("auto", "The original file. Encore measures your connection while it plays and steps the stream down as far as it needs to keep the music playing – down to 96 kbps (64 kbps only on a very poor connection) – then back up as soon as the connection allows."),
    ("original", "Always the original file, even if it stutters on a slow connection. Formats a browser can't play are converted to FLAC (lossless)."),
    ("saver", "Always a small 96 kbps stream (AAC, or Opus in browsers without AAC) – for mobile data and slow links."),
];

/// `seg()` of the JS renderPlayback: compares as strings, so the numeric kbps matches its button.
fn seg_str(p: &Value, key: &str, opts: &[(&str, &str)]) -> String {
    let cur = js_string(p.g(key));
    format!(r##"<div class="seg" data-key="{key}">{}</div>"##,
        opts.iter().map(|(v, l)| format!(r##"<button type="button" data-v="{v}" class="{}">{l}</button>"##, if cur == *v { "on" } else { "" })).collect::<String>())
}

/// `JSON.stringify([link?.id, link?.name, link?.state?.engine ?? null])`.
fn engine_key() -> String {
    let link = crate::player::linked();
    let eng = link.as_ref().map(|l| l.state.g("engine").clone()).unwrap_or(Value::Null);
    json!([link.as_ref().map(|l| l.id.clone()), link.as_ref().map(|l| l.name.clone()), eng]).to_string()
}

// Settings → Playback → Cadenza engine, when there is no engine to show (the same words in web/js/pages/settings.js)
const ENGINE_GREYED: &str = r#"<div class="stack" style="gap:8px;opacity:.45;pointer-events:none" aria-disabled="true"><label class="row" style="gap:8px;align-items:center"><input type="checkbox" disabled> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label><label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" disabled><option>–</option></select></label></div><span class="small muted">Cadenza is Encore's own playback engine: long upsampling filters, DSD, dither and noise shaping, exclusive output straight into the DAC. It is built into the Encore desktop app – install the app on the computer connected to your DAC and this section comes alive on its own. Cadenza can also run on its own on a small box next to the DAC; link to it under Your devices. To have the server do the upsampling for a browser or phone instead, see Server processing above.</span>"#;

/// `${(Number(v) || 0).toFixed(1)} dB`.
fn db_text(n: f64) -> String { let n = if n.is_finite() && n != 0.0 { n } else { 0.0 }; format!("{n:.1} dB") }
/// `Number(v) || 0` printed like JS.
fn js_num_of(v: &Value) -> String { let n = v.as_f64().unwrap_or(0.0); js_string(&json!(n)) }
/// A numeric string as a JSON number (whole numbers as integers, like `JSON.stringify(Number(s))`), else None.
fn eng_num(t: &str) -> Option<Value> {
    let body = t.strip_prefix('-').unwrap_or(t);
    let (int, frac) = body.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((body, None));
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || frac.is_some_and(|f| !digits(f)) { return None; }
    let n: f64 = t.parse().ok()?;
    Some(if n.fract() == 0.0 { json!(n as i64) } else { json!(n) })
}

/// A linked Cadenza describes every switch in its state (see Cadenza's encore/state.rs): outputs, exclusive, bit-perfect, PCM and DSD groups, headroom, soft clipper, GPU, matrix.
fn engine_html(e: &Value) -> String {
    let pair = |o: &Value| -> (Value, Value) { (o.get(0).cloned().unwrap_or(Value::Null), o.get(1).cloned().unwrap_or(Value::Null)) };
    let sel = |key: &str, label: &str, list: &Value, cur: &Value, extra: &str| -> String {
        let Some(items) = list.as_array() else { return String::new() };
        let cur = js_string(cur);
        let opts: String = items.iter().map(|o| {
            let (v, l) = pair(o);
            format!(r#"<option value="{}" {}>{}</option>"#, esc(&js_string(&v)), if js_string(&v) == cur { "selected" } else { "" }, esc(&js_string(&l)))
        }).collect();
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">{label}</span><select class="input" data-eng="{key}" {extra}>{opts}</select></label>"#)
    };
    let seg_e = |key: &str, list: &[Value], cur: &Value| -> String {
        let cur = js_string(cur);
        format!(r#"<div class="seg" data-eng-seg="{key}">{}</div>"#, list.iter().map(|o| {
            let (v, l) = pair(o);
            format!(r#"<button type="button" data-v="{}" class="{}">{}</button>"#, esc(&js_string(&v)), if js_string(&v) == cur { "on" } else { "" }, esc(&js_string(&l)))
        }).collect::<String>())
    };
    let range = |key: &str, label: &str, id: &str, v: &Value, min: &str| -> String {
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">{label}: <span id="{id}">{}</span></span><input type="range" min="{min}" max="0" step="0.5" value="{}" data-eng="{key}" data-v="{id}" style="accent-color:var(--accent)"></label>"#,
            db_text(v.as_f64().unwrap_or(0.0)), js_num_of(v))
    };
    let bp = if e.truthy("bitperfect") { "disabled" } else { "" };
    let dsd = e.o("dsd");
    let manual = !e.g("headroom_db").is_null();
    let gpu_name = if e.truthy("gpu_name") { e.s("gpu_name") } else { "NVIDIA GPU".to_string() };
    let gpu_line = if e.truthy("gpu_in_use") { format!("In use: {}", esc(&gpu_name)) } else if e.truthy("gpu_available") { format!("{} found, not in use", esc(&gpu_name)) } else { "No NVIDIA GPU found".to_string() };
    let exclusive_on = if !e.g("exclusive_wanted").is_null() { e.truthy("exclusive_wanted") } else { e.truthy("exclusive") };
    let exclusive = if e.has("exclusive") { format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="exclusive" {}> Exclusive – Cadenza alone uses the DAC, nothing else is mixed in</label>"#, if exclusive_on { "checked" } else { "" }) } else { String::new() };
    let modes = if e.g("output_modes").is_array() { format!(r#"<div class="stack" style="gap:4px"><span class="small muted">Output mode</span>{}</div>"#, seg_e("output_mode", &e.a("output_modes"), e.g("output_mode"))) } else { String::new() };
    let group = if e.g("output_mode").as_str() == Some("dsd") {
        format!("{}\n      {}\n      {}\n      {}\n      {}",
            sel("dsd_rate", "DSD rate", dsd.g("rates"), dsd.g("rate"), bp),
            sel("dsd_modulator", "DSD modulator", dsd.g("modulators"), dsd.g("modulator"), bp),
            sel("dsd_packing", "DSD packing", dsd.g("packings"), dsd.g("packing"), bp),
            if dsd.g("oversamplings").is_array() { format!(r#"<div class="stack" style="gap:4px"><span class="small muted">DSD oversampling</span>{}</div>"#, seg_e("dsd_oversampling", &dsd.a("oversamplings"), dsd.g("oversampling"))) } else { String::new() },
            if dsd.has("gain_db") { range("dsd_gain_db", "DSD gain", "eng-dsd-gain-v", dsd.g("gain_db"), "-6") } else { String::new() })
    } else {
        let bits: Value = if e.g("bits_list").is_array() { Value::Array(e.a("bits_list").iter().map(|b| json!([b, format!("{}-bit", js_string(b))])).collect()) } else { Value::Null };
        format!("{}\n      {}\n      {}\n      {}",
            sel("filter", "Upsampling filter", e.g("filters"), e.g("filter"), bp),
            sel("dither", "Dither / noise shaping", e.g("dithers"), e.g("dither"), bp),
            sel("rate", "Output rate", e.g("rates"), e.g("rate"), bp),
            sel("bits", "Bit depth", &bits, e.g("bits"), bp))
    };
    let headroom = if e.has("headroom_db") {
        format!("<label class=\"row\" style=\"gap:8px;align-items:center\"><input type=\"checkbox\" id=\"eng-headroom-auto\" {}> Automatic headroom – room for the filters, so loud peaks do not clip</label>\n      {}",
            if manual { "" } else { "checked" },
            if manual { range("headroom_db", "Headroom", "eng-headroom-v", e.g("headroom_db"), "-12") } else { String::new() })
    } else { String::new() };
    let soft = if e.has("soft_clip") { format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="soft_clip" {}> Soft clipper – round off peaks that would clip instead of cutting them</label>"#, if e.truthy("soft_clip") { "checked" } else { "" }) } else { String::new() };
    let gpu = if e.has("gpu") { format!(r#"<div class="stack" style="gap:4px"><span class="small muted">GPU</span>{}<span class="small muted">{gpu_line}</span></div>"#, seg_e("gpu", &[json!(["auto", "Auto"]), json!(["on", "On"]), json!(["off", "Off"])], e.g("gpu"))) } else { String::new() };
    let matrix_cur = if e.g("matrix").is_null() { json!("off") } else { e.g("matrix").clone() };
    format!(r#"<div class="stack" style="gap:8px">
      {output}
      {exclusive}
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" data-eng="bitperfect" {bp_on}> Bit-perfect – send the file to the DAC untouched (no upsampling, dither or volume)</label>
      {modes}
      {group}
      {headroom}
      {soft}
      {gpu}
      {matrix}
      <span class="small muted">Output: {out} · {excl}</span>
      {error}
    </div>"#,
        output = sel("output", "Output device", e.g("outputs"), e.g("output_id"), ""),
        bp_on = if e.truthy("bitperfect") { "checked" } else { "" },
        matrix = sel("matrix", "Matrix (channel mixing)", e.g("matrix_presets"), &matrix_cur, ""),
        out = esc(&if e.truthy("output") { e.s("output") } else { "–".to_string() }),
        excl = if e.truthy("exclusive") { "Exclusive" } else { "Shared" },
        error = if e.truthy("error") { format!(r#"<span class="small" style="color:var(--err)">{}</span>"#, esc(&e.s("error"))) } else { String::new() })
}


const DSP_RATES: [i64; 8] = [88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000];

/// `opt()` of the JS renderPlayback: compares as strings, so a numeric rate matches its option.
fn dsp_opt(v: &Value, l: &Value, cur: &Value) -> String {
    format!(r#"<option value="{}" {}>{}</option>"#, escv(v), if js_string(v) == js_string(cur) { "selected" } else { "" }, escv(l))
}

/// `dspHtml()` of the JS renderPlayback: the Server processing section, greyed out unless switched on and allowed.
fn dsp_html(p: &Value, dsp: &Value) -> String {
    let greyed = r#"<div class="stack" style="gap:8px;opacity:.45;pointer-events:none" aria-disabled="true"><label class="row" style="gap:8px;align-items:center"><input type="checkbox" disabled> Upsample my streams on the server</label><label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" disabled><option>–</option></select></label><label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" disabled><option>–</option></select></label></div>"#;
    if !dsp.truthy("enabled") { return format!(r#"{greyed}<span class="small muted">Turned off on this server. Cadenza upsampling for streams uses the server's processor, so an admin has to switch it on (Settings → Server processing) and allow your account.</span>"#); }
    if !dsp.truthy("allowed") { return format!(r#"{greyed}<span class="small muted">Your account isn't allowed to use the server's processing. Ask an admin.</span>"#); }
    let dev = crate::player::device_rate();
    let off = if p.truthy("dsp") { "" } else { "disabled" };
    let max_rate = dsp.f("max_rate");
    let max_rate = if max_rate.is_nan() { 0.0 } else { max_rate };
    let mut rates: Vec<(Value, String)> = vec![
        (json!("device"), if dev != 0.0 && !dev.is_nan() { format!("Match this device ({} kHz)", crate::player::khz(dev)) } else { "Match this device".to_string() }),
        (json!("auto"), "Highest the server allows".to_string()),
    ];
    rates.extend(DSP_RATES.iter().filter(|r| (**r as f64) <= max_rate).map(|r| (json!(r), format!("{} kHz", crate::player::khz(*r as f64)))));
    let pairs = |key: &str, cur: &Value| -> String {
        dsp.a(key).iter().map(|o| {
            let it = o.items();
            dsp_opt(it.first().unwrap_or(&Value::Null), it.get(1).unwrap_or(&Value::Null), cur)
        }).collect()
    };
    [
        format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-dsp" {}> Upsample my streams on the server</label>"#, if p.truthy("dsp") { "checked" } else { "" }),
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">Upsampling filter</span><select class="input" id="pb-dsp-filter" {off}>{}{}</select></label>"#,
            dsp_opt(&json!(""), &json!("Server default"), p.g("dsp_filter")), pairs("filters", p.g("dsp_filter"))),
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">Dither / noise shaping</span><select class="input" id="pb-dsp-dither" {off}>{}</select></label>"#, pairs("dithers", p.g("dsp_dither"))),
        format!(r#"<label class="stack" style="gap:4px"><span class="small muted">Output rate</span><select class="input" id="pb-dsp-rate" {off}>{}</select></label>"#,
            rates.iter().map(|(v, l)| dsp_opt(v, &json!(l), p.g("dsp_rate"))).collect::<String>()),
        r#"<span class="small muted">The server's Cadenza upsamples the original file and sends lossless 24-bit FLAC. It uses more data (about 4.6 Mbit/s at 96 kHz, 9.2 Mbit/s at 192 kHz) and the server's processor; Data saver and Auto still step down to a lighter stream when the connection can't keep up.</span>"#.to_string(),
    ].join("\n        ")
}

/// `${v > 0 ? '+' : ''}${v} dB`
fn sign_db(v: f64) -> String { format!("{}{} dB", if v > 0.0 { "+" } else { "" }, v) }
fn fade_text(v: f64) -> String { if v > 0.0 { format!("{v} s") } else { "Off".into() } }
/// `+input.value` as a JSON number (NaN → null, which clean() turns into the default).
fn num_json(s: &str) -> Value { let t = s.trim(); let n = if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) }; crate::playback::jnum(n) }
fn num_of(s: &str) -> f64 { let t = s.trim(); if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) } }
/// The bands on screen: the preset's own until you edit one (then the preset is Custom).
fn sound_bands(p: &Value) -> Vec<Value> { crate::playback::eq_bands(p) }
fn band_row(b: &Value, i: usize) -> String {
    let n = i + 1;
    format!(r#"<tr data-i="{i}">
      <td><select class="input" data-f="type" aria-label="Band {n} type">{types}</select></td>
      <td><input class="input" type="number" data-f="freq" min="20" max="20000" step="1" value="{freq}" aria-label="Band {n} frequency (Hz)"></td>
      <td><input class="input" type="number" data-f="gain" min="-24" max="24" step="0.5" value="{gain}" aria-label="Band {n} gain (dB)"></td>
      <td><input class="input" type="number" data-f="q" min="0.1" max="18" step="0.1" value="{q}" aria-label="Band {n} Q"></td>
      <td><button type="button" class="btn ghost icon sm" data-rm-band="{i}" aria-label="Remove band" title="Remove band">✕</button></td></tr>"#,
        types = crate::playback::EQ_TYPES.iter().map(|(v, l)| dsp_opt(&json!(v), &json!(l), b.g("type"))).collect::<String>(),
        freq = js_string(b.g("freq")), gain = js_string(b.g("gain")), q = js_string(b.g("q")))
}
fn sound_html(p: &Value) -> String {
    let list = sound_bands(p);
    let on = p.truthy("fx_on");
    let presets: String = crate::playback::EQ_PRESETS.iter().map(|x| dsp_opt(&json!(x.id), &json!(x.label), p.g("eq_preset"))).collect::<String>()
        + &if p.s("eq_preset") == "custom" { dsp_opt(&json!("custom"), &json!("Custom"), p.g("eq_preset")) } else { String::new() };
    let rows = if list.is_empty() { r#"<tr><td colspan="5" class="small muted">No bands.</td></tr>"#.to_string() } else { list.iter().enumerate().map(|(i, b)| band_row(b, i)).collect() };
    format!(r#"<label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-fx" {fx_checked}> Use sound effects</label>
      <fieldset class="stack" id="pb-fx-box" style="gap:8px;border:0;padding:0;margin:0;min-width:0;{fx_style}" {fx_disabled}>
        <label class="stack" style="gap:4px"><span class="small muted">Equalizer preset</span><select class="input" id="pb-eq-preset">{presets}</select></label>
        <label class="stack" style="gap:4px"><span class="small muted">Preamp <b id="pb-eq-preamp-v">{preamp_text}</b></span><input type="range" id="pb-eq-preamp" min="-24" max="12" step="0.5" value="{preamp}"></label>
        {graph}
        <span class="small muted">Drag a point to move it (left/right = frequency, up/down = gain), scroll on a bell to change its width (Q), double-click the graph to add a bell there.</span>
        <div style="overflow-x:auto"><table class="eq-bands" style="width:100%;border-collapse:collapse">
          <thead><tr class="small muted" style="text-align:left"><th>Type</th><th>Frequency (Hz)</th><th>Gain (dB)</th><th>Q</th><th></th></tr></thead>
          <tbody>{rows}</tbody></table></div>
        <div class="row"><button type="button" class="btn sm" id="pb-eq-add" {add_disabled}>{plus}Add band</button></div>
        <label class="stack" style="gap:4px"><span class="small muted">Crossfeed</span><select class="input" id="pb-crossfeed">{crossfeed}</select></label>
        <span class="small muted">For headphones: mixes a little of each channel into the other, like speakers in a room.</span>
        <label class="stack" style="gap:4px"><span class="small muted">Balance <b id="pb-balance-v">{balance_text}</b></span><input type="range" id="pb-balance" min="-100" max="100" step="1" value="{balance}"></label>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-mono" {mono}> Mono</label>
      </fieldset>
      <span class="small muted">Runs in this browser or app – it costs your server nothing. Works with every quality setting. Turning it on means the sound is no longer the untouched file (the Signal path says so).</span>"#,
        graph = super::eq_graph::html(&list, on),
        fx_checked = if on { "checked" } else { "" },
        fx_style = if on { "" } else { "opacity:.45" },
        fx_disabled = if on { "" } else { "disabled" },
        preamp_text = sign_db(p.f("eq_preamp")), preamp = js_string(p.g("eq_preamp")),
        add_disabled = if list.len() >= super::eq_graph::MAX { "disabled" } else { "" }, plus = icon("plus"),
        crossfeed = crate::playback::CROSSFEED.iter().map(|(v, l)| dsp_opt(&json!(v), &json!(l), p.g("crossfeed"))).collect::<String>(),
        balance_text = crate::playback::balance_text(p.f("balance")), balance = js_string(p.g("balance")),
        mono = if p.truthy("mono") { "checked" } else { "" })
}
/// Song transitions: how one song becomes the next (every mode uses the seconds and the album rule).
fn transition_help(mode: &str) -> &'static str {
    match mode {
        "off" => "Each song plays to its end, then the next one starts.",
        "crossfade" => "The end of one song fades out while the next one fades in.",
        "overlap" => "The next song starts at full volume a few seconds early while the old one fades out under it.",
        "smart" => "Skips the silence at the end of one song and the start of the next, then overlaps them from the last audible moment, with a short fade.",
        _ => "",
    }
}
fn transitions_html(p: &Value, cadenza: bool) -> String {
    let off = p.s("transition") == "off";
    let cf = p.f("crossfade");
    let secs = if cf > 0.0 { cf } else { crate::playback::TRANSITION_DEFAULT_S }.max(1.0);
    format!(r#"{seg}
      <span class="small muted">{help}</span>
      <label class="stack" style="gap:4px;{dim}"><span class="small muted">Length <b id="pb-crossfade-v">{text}</b></span><input type="range" id="pb-crossfade" min="1" max="12" step="1" value="{secs}" {disabled}></label>
      <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-crossfade-gapless" {gapless}> Keep albums gapless (no transition between songs from the same album)</label>
      {note}"#,
        seg = seg_str(p, "transition", &crate::playback::TRANSITIONS),
        help = transition_help(&p.s("transition")),
        dim = if off { "opacity:.45" } else { "" },
        text = fade_text(secs),
        disabled = if off { "disabled" } else { "" },
        gapless = if p.truthy("crossfade_album_gapless") { "checked" } else { "" },
        note = if cadenza { r#"<span class="small muted">Song transitions don't apply while playing through Cadenza</span>"# } else { "" })
}
fn bind_sound(st: &Rc<Look>) {
    let el = st.el.clone();
    let check = |sel: &str, key: &'static str| {
        if let Some(c) = dom::qs_in(&el, sel) {
            let (st2, c2) = (st.clone(), c.clone());
            dom::on(&c, "change", move |_| set_playback(&st2, json!({ key: dom::checked(&c2) })));
        }
    };
    check("#pb-fx", "fx_on");
    check("#pb-mono", "mono");
    check("#pb-crossfade-gapless", "crossfade_album_gapless");
    if let Some(c) = dom::qs_in(&el, "#pb-eq-preset") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| {
            let v = dom::value(&c2);
            if let Some(x) = crate::playback::EQ_PRESETS.iter().find(|y| y.id == v) {
                set_playback(&st2, json!({ "eq_preset": x.id, "eq_bands": crate::playback::preset_bands_json(x), "eq_preamp": crate::playback::jnum(x.preamp) }));
            }
        });
    }
    // a range slider: its value shows while dragging, and is saved when let go
    let slider = |sel: &str, label: &'static str, key: &'static str, text: fn(f64) -> String| {
        let Some(r) = dom::qs_in(&el, sel) else { return };
        let (el2, r2) = (el.clone(), r.clone());
        dom::on(&r, "input", move |_| { if let Some(v) = dom::qs_in(&el2, label) { dom::set_text(&v, &text(num_of(&dom::value(&r2)))); } });
        let (st2, r2) = (st.clone(), r.clone());
        dom::on(&r, "change", move |_| set_playback(&st2, json!({ key: num_json(&dom::value(&r2)) })));
    };
    slider("#pb-eq-preamp", "#pb-eq-preamp-v", "eq_preamp", sign_db);
    slider("#pb-balance", "#pb-balance-v", "balance", crate::playback::balance_text);
    slider("#pb-crossfade", "#pb-crossfade-v", "crossfade", fade_text);
    {
        let (p, st2) = (st.p.borrow().clone(), st.clone());
        super::eq_graph::bind(&el, sound_bands(&p), p.truthy("fx_on"), move |list| set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list })));
    }
    {
        let st2 = st.clone();
        dom::on_each(&el, ".eq-bands tr[data-i] [data-f]", "change", move |c, _| {
            let mut list = sound_bands(&st2.p.borrow());
            let Some(tr) = dom::closest_of(c, "tr") else { return };
            let i = num_of(&dom::data(&tr, "i")) as usize;
            let f = dom::data(c, "f");
            let v = if f == "type" { json!(dom::value(c)) } else { num_json(&dom::value(c)) };
            if let Some(b) = list.get_mut(i) { b[f.as_str()] = v; }
            set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list }));
        });
    }
    {
        let st2 = st.clone();
        dom::on_each(&el, "[data-rm-band]", "click", move |b, _| {
            let mut list = sound_bands(&st2.p.borrow());
            let i = num_of(&dom::data(b, "rmBand")) as usize;
            if i < list.len() { list.remove(i); }
            set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list }));
        });
    }
    if let Some(b) = dom::qs_in(&el, "#pb-eq-add") {
        let st2 = st.clone();
        dom::on(&b, "click", move |_| {
            let mut list = sound_bands(&st2.p.borrow());
            if list.len() < super::eq_graph::MAX {
                list.push(json!({ "type": "peaking", "freq": 1000, "gain": 0, "q": 1 }));
                set_playback(&st2, json!({ "eq_preset": "custom", "eq_bands": list }));
            }
        });
    }
    if let Some(c) = dom::qs_in(&el, "#pb-crossfeed") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "crossfeed": dom::value(&c2) })));
    }
}

fn paint_playback(st: &Rc<Look>) {
    let p = st.p.borrow().clone();
    let link = crate::player::linked();
    let eng = link.as_ref().map(|l| l.state.g("engine").clone()).filter(crate::json::truthy);
    let help = QUALITY_HELP.iter().find(|(k, _)| *k == p.s("quality")).map(|(_, h)| *h).unwrap_or("undefined");
    *st.painted.borrow_mut() = engine_key();
    let eng_title = match (&eng, &link) {
        (Some(_), Some(l)) => format!(" · {}", esc(&l.name)),
        _ => String::new(),
    };
    let engine_block = match (&eng, &link) {
        (Some(e), Some(_)) => engine_html(e),
        // no linked Cadenza: greyed out
        _ => ENGINE_GREYED.to_string(),
    };
    dom::set_html(&st.el, &format!(r##"<div class="card stack">
      <h2>Playback</h2>
      <p class="small muted" style="margin:0">{saved}</p>
      <div class="stack" style="gap:6px"><h3>Quality</h3>{quality}
        <span class="small muted">{help}</span></div>
      <div class="stack" style="gap:6px"><h3>Buffering</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-preload" {preload}> Pre-load the next song while this one plays</label>
        <span class="small muted">The next song is fetched once the current one is safely buffered, so it starts without a gap. Turn it off on a very slow connection to leave all the bandwidth to the song playing.</span></div>
      <div class="stack" style="gap:6px"><h3>When the queue ends</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-autoplay" {autoplay}> Keep going with a shuffle of the whole library</label>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-resume" {resume}> Resume where I left off when the app opens</label></div>
      <div class="stack" style="gap:6px"><h3>Shuffle</h3>
        <label class="row" style="gap:8px;align-items:center"><input type="checkbox" id="pb-smart" {smart}> Smart shuffle</label>
        <span class="small muted">Shuffles and the music after the queue lean to what you play and like: favourites come up more often, songs you just heard wait, songs you skip a lot come up less, and songs or artists you disliked never play. Off, a shuffle is plain random.</span></div>
      <div class="stack" style="gap:6px"><h3>Sound (on this device)</h3>
        {sound}</div>
      <div class="stack" style="gap:6px"><h3>Song transitions</h3>
        {transitions}</div>
      <div class="stack" style="gap:6px"><h3>Pure playback</h3>
        <span class="small muted">Encore streams the original file untouched – the quality badge on the player opens the <b>Signal path</b>, which shows what the file is, what was sent and what this device does with it. A browser hands the sound to the operating system, which mixes it with other sounds and may resample it; bit-perfect output needs an exclusive-mode engine.</span></div>
      <div class="stack" style="gap:6px"><h3>Server processing (Cadenza)</h3>
        {dsp_block}</div>
      <div class="stack" style="gap:6px"><h3>Cadenza engine{eng_name}</h3>
        {engine_block}</div>
    </div>"##,
        saved = if st.server_prefs { "Saved to your account, so it follows you to every device." } else { "Saved on this device." },
        quality = seg_str(&p, "quality", &[("auto", "Auto"), ("original", "Original"), ("saver", "Data saver")]),
        preload = if p.truthy("preload") { "checked" } else { "" },
        autoplay = if p.truthy("autoplay") { "checked" } else { "" },
        resume = if p.truthy("resume") { "checked" } else { "" },
        smart = if p.truthy("smart_shuffle") { "checked" } else { "" },
        sound = sound_html(&p),
        transitions = transitions_html(&p, eng.is_some()),
        dsp_block = dsp_html(&p, &st.dsp),
        eng_name = eng_title));
    {
        let st2 = st.clone();
        dom::on_each(&st.el, ".seg[data-key] button", "click", move |b, _| {
            let Some(sg) = dom::closest_of(b, ".seg") else { return };
            set_playback(&st2, json!({ dom::data(&sg, "key"): dom::data(b, "v") }));
        });
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-preload") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "preload": dom::checked(&c2) })));
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-autoplay") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "autoplay": dom::checked(&c2) })));
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-smart") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "smart_shuffle": dom::checked(&c2) })));
    }
    if let Some(c) = dom::qs_in(&st.el, "#pb-resume") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "resume": dom::checked(&c2) })));
    }
    bind_sound(st);
    // Volume normalization (player/norm.rs): first in Sound (on this device), before "Use sound effects"
    crate::player::norm_setting(&st.el);
    crate::player::offline_setting(&st.el);
    if let Some(c) = dom::qs_in(&st.el, "#pb-dsp") {
        let (st2, c2) = (st.clone(), c.clone());
        dom::on(&c, "change", move |_| set_playback(&st2, json!({ "dsp": dom::checked(&c2) })));
        for (sel, key) in [("#pb-dsp-filter", "dsp_filter"), ("#pb-dsp-dither", "dsp_dither"), ("#pb-dsp-rate", "dsp_rate")] {
            let Some(c) = dom::qs_in(&st.el, sel) else { continue };
            let (st2, c2) = (st.clone(), c.clone());
            dom::on(&c, "change", move |_| {
                let v = dom::value(&c2);
                // the rate is stored as a number when it is one
                let v = if key == "dsp_rate" && !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) { v.parse::<f64>().map(|n| json!(n as i64)).unwrap_or_else(|_| json!(v)) } else { json!(v) };
                set_playback(&st2, json!({ key: v }));
            });
        }
    }
    if let Some(l) = link {
        // every engine switch goes to the linked device as { action: 'engine', set: { key: value } }
        let l = Rc::new(l);
        let send_eng: Rc<dyn Fn(Value)> = Rc::new(move |set| {
            (l.send)(json!({ "action": "engine", "set": set }));
            toast(&format!("Sent to {}", l.name), "ok");
        });
        for c in dom::qsa_in(&st.el, "[data-eng]") {
            if c.get_attribute("type").as_deref() != Some("range") { continue; }
            let (el, c2) = (st.el.clone(), c.clone());
            dom::on(&c, "input", move |_| {
                if let Some(v) = dom::qs_in(&el, &format!("#{}", dom::data(&c2, "v"))) { dom::set_text(&v, &db_text(dom::value(&c2).parse().unwrap_or(0.0))); }
            });
        }
        {
            let send_eng = send_eng.clone();
            dom::on_each(&st.el, "[data-eng]", "change", move |c, _| {
                let k = dom::data(c, "eng");
                let ty = c.get_attribute("type").unwrap_or_default();
                let raw = dom::value(c);
                let v = if ty == "checkbox" { json!(dom::checked(c)) }
                    else if ty == "range" { eng_num(&raw).unwrap_or_else(|| json!(0)) }
                    else if k == "rate" || k == "bits" { eng_num(&raw).unwrap_or_else(|| json!(raw)) }
                    else { json!(raw) };
                send_eng(json!({ k: v }));
            });
        }
        {
            let send_eng = send_eng.clone();
            dom::on_each(&st.el, "[data-eng-seg] button", "click", move |b, _| {
                let Some(sg) = dom::closest_of(b, ".seg") else { return };
                send_eng(json!({ dom::data(&sg, "engSeg"): dom::data(b, "v") }));
            });
        }
        if let Some(ha) = dom::qs_in(&st.el, "#eng-headroom-auto") {
            let ha2 = ha.clone();
            dom::on(&ha, "change", move |_| send_eng(json!({ "headroom_db": if dom::checked(&ha2) { Value::Null } else { json!(-3) } })));
        }
    }
}

fn set_playback(st: &Rc<Look>, patch: Value) {
    let p = {
        let mut cur = st.p.borrow_mut();
        if let (Some(dst), Some(src)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in src { dst.insert(k.clone(), v.clone()); }
        }
        cur.clone()
    };
    let server_prefs = st.server_prefs;
    let st2 = st.clone();
    dom::spawn(async move {
        let c = crate::playback::save(&p, server_prefs).await;
        *st2.p.borrow_mut() = c;
        paint_playback(&st2);
    });
    paint_playback(st);
}
