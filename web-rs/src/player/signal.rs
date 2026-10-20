//! Signal path: what the file is, what Encore sends to this device and what this device does with it.

use super::*;

thread_local! {
    /// track id → /info (codec, sample rate, bit depth …)
    pub(super) static INFO: RefCell<HashMap<i64, Value>> = RefCell::new(HashMap::new());
}

// ---- signal path: what the file is, what Encore sends, what this device does with it -----------------
pub(super) async fn track_info(t: &Value) -> Option<Value> {
    if !t.g("id").is_number() { return None; }
    let id = t.i("id");
    if let Some(i) = INFO.with(|m| m.borrow().get(&id).cloned()) { return Some(i); }
    match api::get(&format!("/library/tracks/{id}/info")).await {
        Ok(i) => { INFO.with(|m| m.borrow_mut().insert(id, i.clone())); Some(i) }
        Err(_) => None,
    }
}
/// What leaves the server for this device: `(kind, text)`.
pub(super) fn transport() -> Option<(String, String)> {
    let t = cur()?;
    if t.truthy("preview") { return Some(("preview".into(), "A 30-second preview from the catalogue (lossy) – not your file".into())); }
    if st(|s| s.local_copy) { return Some(("local".into(), format!("Downloaded copy on this device · {}", copy_quality(&t)))); }
    let rung = st(|s| s.rung);
    if rung > 0 {
        let codec = lossy_codec();
        let why = if prefs().s("quality") == "saver" { "(Data saver is on)" } else { "(adaptive – the connection could not keep up)" };
        return Some((format!("{codec}-{rung}"), format!("{} {rung} kbps, lossy {why}", codec.to_uppercase())));
    }
    if st(|s| s.dsp) {
        let r = st(|s| s.dsp_rate);
        return Some((format!("cadenza-{}", js_num(r)), format!("Upsampled on the server by Cadenza to 24-bit / {} kHz (lossless FLAC){}", khz(r), dsp_note(&dsp_state(t.i("id"))))));
    }
    if st(|s| s.transcoded) { return Some(("flac".into(), format!("Converted to FLAC by the server (lossless) – browsers can't play {}", t.s("format").to_uppercase()))); }
    Some(("original".into(), "The original file, byte for byte – nothing was converted or resampled on the way".into()))
}

/// kbit/s → "0.25" / "4.2" / "12" (Mbit/s)
fn mbit(kbps: f64) -> String {
    if kbps < 1000.0 { format!("{:.2}", kbps / 1000.0) } else if kbps < 10000.0 { to_fixed1(kbps / 1000.0) } else { js_num(playback::js_round(kbps / 1000.0)) }
}
fn source_line(t: &Value, info: Option<&Value>) -> String {
    let Some(info) = info else {
        return format!("{}{}", t.s("format").to_uppercase(), if t.truthy("bitrate") { format!(" · {} kbps", t.s("bitrate")) } else { String::new() });
    };
    let mut parts = vec![(if info.truthy("codec") { info.s("codec") } else { t.s("format") }).to_uppercase()];
    if info.truthy("lossless") && info.truthy("bit_depth") && info.truthy("sample_rate") { parts.push(format!("{}-bit / {} kHz", info.s("bit_depth"), khz(info.f("sample_rate")))); }
    else if info.truthy("sample_rate") { parts.push(format!("{} kHz", khz(info.f("sample_rate")))); }
    if info.truthy("channels") {
        let ch = info.i("channels");
        parts.push(if ch == 2 { "stereo".into() } else if ch == 1 { "mono".into() } else { format!("{ch} channels") });
    }
    if info.truthy("bitrate") { parts.push(format!("{} kbps", info.s("bitrate"))); }
    parts.join(" · ")
}
/// The Signal path window: three rows, and a plain verdict on whether what you hear is the file untouched.
pub(super) async fn signal_path() {
    let Some(t) = now_track() else { return };
    let m = crate::ui::modal(ModalOpts::new("Signal path", r#"<div class="skeleton" style="height:120px"></div>"#));
    let info = track_info(&t).await;
    if linked().is_none() && st(|s| s.dsp) && dsp_cache_on() && cur().is_some_and(|c| *c == t) { prepare_dsp(&t).await; }   // pre-rendered yet?
    let mut rows: Vec<(String, String, String)> = vec![];
    // quality checks (fake lossless, upsampled, very quiet…): "Fake lossless? cutoff 16 kHz"
    let suspect = info.as_ref().map(|i| i.o("quality")).filter(|q| q.s("status") == "suspect").map(|q| q.s("summary")).unwrap_or_default();
    let lossy_note = if info.as_ref().is_some_and(|i| i.g("lossless") == &json!(false)) { "A lossy file: what the file lost is gone before Encore touches it".to_string() } else { String::new() };
    let linked = linked();
    let copy = linked.is_none() && st(|s| s.local_copy) && cur().is_some_and(|c| *c == t);
    let converted = copy && copy_converted(&t);
    if copy {
        // the saved copy: where the music comes from now; the server's file only as a note
        let note = if converted { format!("Converted from the server's file: {}", source_line(&t, info.as_ref())) } else { [lossy_note, suspect].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ") };
        rows.push(("Source".into(), format!("Downloaded copy on this device · {}", copy_quality(&t)), note));
    } else {
        rows.push(("Source".into(), source_line(&t, info.as_ref()), [lossy_note, suspect].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")));
    }
    let pure;
    let mut untouched = false;
    // volume normalization changes this song's level here (so it isn't "Pure playback")
    let mut normed = false;
    let cadenza = linked.is_none() && transport().is_some_and(|(k, _)| k.starts_with("cadenza"));
    // a Cadenza device reports every stage of its own pipeline; a browser device just plays
    let stages: Vec<Value> = linked.as_ref().map(|l| l.state.g("signal").a("stages")).unwrap_or_default();
    if let Some(l) = &linked {
        let sig = l.state.o("signal");
        let eng = l.state.o("engine");
        let tr = sig.s("transport");
        let what = if tr == "original" { "The original file, byte for byte".to_string() }
            else if tr.is_empty() { "Not reported by that device".to_string() }
            else if tr == "flac" { "Converted to FLAC by the server (lossless)".to_string() }
            else if tr == "local" { "Nothing: it plays a copy downloaded to that device".to_string() }
            else if let Some(rate) = tr.strip_prefix("cadenza-") { let n = num(rate); format!("Upsampled on the server by Cadenza ({} kHz)", khz(if truthy_num(n) { n } else { 0.0 })) }
            else if tr.starts_with("aac-") || tr.starts_with("opus-") { let (c, k) = tr.split_once('-').unwrap_or((&tr, "")); format!("{} {k} kbps (lossy)", c.to_uppercase()) }
            else { tr.clone() };
        rows.push((format!("Encore → {}", esc(&l.name)), what, String::new()));
        if !stages.is_empty() {
            for stg in &stages { rows.push((esc(&stg.s("name")), stg.s("detail"), String::new())); }
            rows.push(("Output".into(), format!("{}{}{}", eng.s("output"), if eng.truthy("exclusive") { " · exclusive" } else { "" }, if sig.truthy("bitperfect") { " · bit-perfect" } else { "" }), String::new()));
        } else {
            rows.push((esc(&l.name), if sig.truthy("output_rate") { format!("Plays through its browser audio at {} kHz", khz(sig.f("output_rate"))) } else { "Playing there".into() }, String::new()));
            // what that device's volume normalization does to the song (its own settings), while it is on there
            if let Some(n) = sig.st("norm") { rows.push(("Normalization".into(), n.to_string(), String::new())); }
        }
        pure = false;
    } else {
        let tr = transport();
        let net = match est_kbps() { Some(k) if k > 0.0 => format!(" · connection ≈ {} Mbit/s", mbit(k)), _ => String::new() };
        if !copy { rows.push(("Encore → this device".into(), format!("{}{net}", tr.as_ref().map(|(_, x)| x.clone()).unwrap_or_default()), String::new())); }
        let out = output_rate();
        let src = info.as_ref().map(|i| i.f("sample_rate")).unwrap_or(0.0);
        let resampled = truthy_num(out) && truthy_num(src) && out != src;
        rows.push(("This device".into(),
            if truthy_num(out) { format!("Browser audio → the system mixer at {} kHz", khz(out)) } else { "Browser audio → the system mixer".into() },
            if resampled { format!("Your device resamples {} → {} kHz. A browser can't hand bits straight to the DAC; for true bit-perfect output play through a Cadenza device or an exclusive-mode player.", khz(src), khz(out)) }
            else if truthy_num(out) && truthy_num(src) { "Same rate as the file – no resampling in the browser. Whether the operating system mixes it with other sounds is up to your audio settings.".into() }
            else { String::new() }));
        // volume normalization: the first stage on this device, before the effects (a row only while it is on)
        if !t.truthy("preview") {
            if let Some((v, note)) = norm_row(&t, info.as_ref()) { rows.push(("Normalization".into(), v, note)); }
            normed = norm_changes(&t);
        }
        let effects = playback::effects_summary(&prefs(), true);
        if !effects.is_empty() { rows.push(("Effects on this device".into(), effects, String::new())); }
        let tr = transport();
        untouched = tr.as_ref().is_some_and(|(k, _)| k == "original") && !info.as_ref().is_some_and(|i| i.g("lossless") == &json!(false));
        pure = untouched && !playback::fx_active(&prefs()) && !normed;
    }
    let Some(body_el) = m.qs(".modal-body") else { return };
    let rows_html: String = rows.iter().map(|(k, v, note)| format!(r#"<div class="stack" style="gap:2px"><div class="small muted">{k}</div><div style="font-weight:600">{}</div>{}</div>"#,
        esc(v), if note.is_empty() { String::new() } else { format!(r#"<div class="small muted">{}</div>"#, esc(note)) })).collect();
    let (head, text) = if let Some(l) = &linked {
        if !stages.is_empty() {
            (format!("Played by Cadenza on {}", esc(&l.name)), "Every stage above is what the engine reports. Bit-perfect means nothing touched the samples.")
        } else {
            ("Playing on another device".to_string(), "The rows above are what that device reports. Open Encore there for its own signal path.")
        }
    } else if cadenza {
        ("Processed by Cadenza on the server".to_string(), "Encore upsampled the file on the server with Cadenza and sent lossless 24-bit FLAC. Turn it off under Settings → Playback → Server processing to get the untouched file.")
    } else if copy && converted {
        ("Not the untouched file".to_string(), "This download is a converted copy: the original file doesn't play in this browser, or the server converted it. Where the original plays, Downloads → ⋯ → Download again in original quality replaces it.")
    } else if copy && playback::fx_active(&prefs()) {
        ("Not the untouched file".to_string(), "The original file, downloaded byte for byte; this device adds the effects you chose under Settings → Playback → Sound.")
    } else if copy && normed {
        ("Not the untouched file".to_string(), "The original file, downloaded byte for byte; this device changes its volume for normalization (Settings → Playback → Sound).")
    } else if copy {
        ("Pure playback from this device".to_string(), "The original file, downloaded byte for byte. Everything after that – volume, mixing, resampling – is your browser and operating system.")
    } else if pure {
        ("Pure playback from Encore".to_string(), "Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is your browser and operating system.")
    } else if untouched && !playback::fx_active(&prefs()) {
        ("Not the untouched file".to_string(), "Encore sent the file untouched; this device changes its volume for normalization (Settings → Playback → Sound).")
    } else if untouched {
        ("Not the untouched file".to_string(), "Encore sent the file untouched; this device adds the effects you chose under Settings → Playback → Sound.")
    } else {
        ("Not the untouched file".to_string(), "Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow.")
    };
    set_html(&body_el, &format!(r#"
    <div class="stack" style="gap:12px">
      {rows_html}
      <div class="card" style="padding:12px"><b>{head}</b>
        <div class="small muted" style="margin-top:4px">{text}</div></div>
    </div>"#));
}
