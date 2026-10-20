//! Sound effects on this device (Web Audio): the graph, its AudioContext and this device's output rate. Each element's
//! first stage is its volume normalization gain (norm.rs), before the preamp and the effects.

use super::*;

thread_local! {
    /// AudioContext: reads the device's output sample rate, and runs the sound effects when they are on
    pub(super) static CTX: RefCell<Option<web_sys::AudioContext>> = const { RefCell::new(None) };
    /// the sound-effects graph (Web Audio)
    pub(super) static FX: RefCell<Fx> = RefCell::new(Fx::default());
}

pub(super) fn fx_built() -> bool { FX.with(|f| f.borrow().built) }

pub(super) fn audio_ctx() -> Option<web_sys::AudioContext> { CTX.with(|c| c.borrow().clone()) }

pub(super) fn output_rate() -> f64 {
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() { *c = web_sys::AudioContext::new().ok(); }
        c.as_ref().map(|x| x.sample_rate() as f64).unwrap_or(0.0)
    })
}

/// This device's output sample rate in Hz (0 if unknown) – Settings → Playback shows it for "Match this device".
pub fn device_rate() -> f64 { output_rate() }

// ---- sound effects on this device (Web Audio) ----------------------------------------------------
// Built the first time an effect or volume normalization is switched on; until then the <audio> elements play
// straight out, untouched. An element wrapped in a MediaElementSource can only be heard through the graph, so once
// built the graph stays and turns into a pass-through when the effects are switched off.
// Each wrapped element: source → normalization gain (the song's own, norm.rs) → fade gain (song transitions) → the
// shared input (the preamp) → EQ, crossfeed, mono, balance → the speakers.
/// `out` feeds the graph: the fade gain, or the last node that could be made · `norm`: the normalization gain
pub(super) struct FxSource { pub(super) el: HtmlMediaElement, out: web_sys::AudioNode, pub(super) gain: Option<web_sys::GainNode>, pub(super) norm: Option<web_sys::GainNode>, on: bool }
#[derive(Default)]
pub(super) struct Fx {
    built: bool,
    /// half built (a node could not be made): what is wrapped plays through a plain pass-through
    broken: bool,
    pub(super) sources: Vec<FxSource>,
    input: Option<web_sys::GainNode>,
    key: String,
    eq: Vec<web_sys::BiquadFilterNode>,
    cross: Vec<web_sys::GainNode>,
    trim: Option<web_sys::GainNode>,
    pan: Option<web_sys::StereoPannerNode>,
    nodes: Vec<web_sys::AudioNode>,
}
fn db_gain(v: f64) -> f64 { 10f64.powf(v / 20.0) }
fn xfeed_db(x: &str) -> f64 { match x { "light" => -9.0, "medium" => -6.0, "strong" => -4.5, _ => -6.0 } }
fn biquad_type(t: &str) -> web_sys::BiquadFilterType {
    use web_sys::BiquadFilterType as B;
    match t { "lowshelf" => B::Lowshelf, "highshelf" => B::Highshelf, "lowpass" => B::Lowpass, "highpass" => B::Highpass, "notch" => B::Notch, _ => B::Peaking }
}

pub(super) fn ensure_fx() -> bool {
    if fx_built() { return true; }
    // a preview playing now stays out of the graph (it is built at the next library song)
    if cur().is_some_and(|t| t.truthy("preview")) { return false; }
    let (Some(a), Some(sp)) = (audio(), spare()) else { return false };
    output_rate();
    let Some(ctx) = audio_ctx() else { return false };
    let built = (|| -> Result<(), JsValue> {
        let input = ctx.create_gain()?;
        FX.with(|f| f.borrow_mut().input = Some(input));
        for el in [a, sp] {
            el.set_cross_origin(Some("anonymous"));
            let src = ctx.create_media_element_source(&el)?;
            // kept at once: an element can be wrapped only once, and from now on is heard only through the graph
            let i = FX.with(|f| { let mut f = f.borrow_mut(); f.sources.push(FxSource { el, out: src.clone().into(), gain: None, norm: None, on: false }); f.sources.len() - 1 });
            let norm = ctx.create_gain()?;
            src.connect_with_audio_node(&norm)?;
            FX.with(|f| { let mut f = f.borrow_mut(); f.sources[i].out = norm.clone().into(); f.sources[i].norm = Some(norm.clone()); });
            let gain = ctx.create_gain()?;
            norm.connect_with_audio_node(&gain)?;
            FX.with(|f| { let mut f = f.borrow_mut(); f.sources[i].out = gain.clone().into(); f.sources[i].gain = Some(gain); });
        }
        Ok(())
    })();
    let ok = built.is_ok();
    let wrapped = FX.with(|f| {
        let mut f = f.borrow_mut();
        if f.sources.is_empty() { f.input = None; return false; }   // nothing wrapped: the elements still play straight out
        f.built = true;
        f.broken = !ok;
        f.key = String::new();
        true
    });
    if !wrapped { return false; }
    if ok { apply_fx(); }
    else if let Some(i) = FX.with(|f| f.borrow().input.clone()) { let _ = i.connect_with_audio_node(&ctx.destination()); }   // half built: a pass-through
    route();
    // the song playing now takes its normalization gain (the next ones when they start)
    norm_now(false);
    resume_fx();
    true
}
/// The graph is needed: an effect changes the sound, or volume normalization is on.
pub(super) fn wants_graph() -> bool { playback::fx_active(&prefs()) || norm_on() }
/// Resume the graph's context on play: suspended by the autoplay policy, or 'interrupted' (Safari after a call or Siri).
pub(super) fn resume_fx() {
    if !fx_built() { return; }
    if let Some(ctx) = audio_ctx() {
        if ctx.state() != web_sys::AudioContextState::Running {
            if let Ok(p) = ctx.resume() { dom::spawn(async move { let _ = wasm_bindgen_futures::JsFuture::from(p).await; }); }
        }
    }
}
/// Only the element playing (and the one fading out) feeds the graph.
pub(super) fn route() {
    let a = audio();
    let out = FADE.with(|f| f.borrow().as_ref().map(|f| f.out.clone()));
    FX.with(|f| {
        let mut f = f.borrow_mut();
        if !f.built { return; }
        let input: web_sys::AudioNode = match (f.input.clone(), audio_ctx()) {
            (Some(i), _) => i.into(),
            (None, Some(c)) => c.destination().into(),
            _ => return,
        };
        for s in f.sources.iter_mut() {
            let want = a.as_ref().is_some_and(|a| same_el(&s.el, a)) || out.as_ref().is_some_and(|o| same_el(&s.el, o));
            if want && !s.on { let _ = s.out.connect_with_audio_node(&input); s.on = true; }
            else if !want && s.on { let _ = s.out.disconnect(); s.on = false; }
        }
    });
}
/// Bring the graph in line with the settings: rewire only when its shape changes, otherwise just set the values.
pub(super) fn apply_fx() {
    if FX.with(|f| { let f = f.borrow(); !f.built || f.broken }) { return; }
    let Some(ctx) = audio_ctx() else { return };
    let p = prefs();
    let on = playback::fx_active(&p);
    let bands = if on { playback::eq_bands(&p) } else { vec![] };
    let mono = on && p.truthy("mono");
    let xfeed = if on && !mono { p.s("crossfeed") } else { "off".to_string() };
    let key = json!([on, bands.iter().map(|b| b.g("type").clone()).collect::<Vec<_>>(), xfeed != "off", mono]).to_string();
    FX.with(|f| {
        let mut fx = f.borrow_mut();
        let Some(input) = fx.input.clone() else { return };
        if key != fx.key {
            fx.key = key;
            let _ = input.disconnect();
            for n in fx.nodes.drain(..) { let _ = n.disconnect(); }
            fx.eq.clear(); fx.cross.clear(); fx.trim = None; fx.pan = None;
            let wired = (|fx: &mut Fx| -> Result<(), JsValue> {
                let mut last: web_sys::AudioNode = input.clone().into();
                let add = |fx: &mut Fx, last: &mut web_sys::AudioNode, n: web_sys::AudioNode| -> Result<(), JsValue> {
                    fx.nodes.push(n.clone());
                    last.connect_with_audio_node(&n)?;
                    *last = n;
                    Ok(())
                };
                if on {
                    for b in &bands {
                        let f = ctx.create_biquad_filter()?;
                        f.set_type(biquad_type(&b.s("type")));
                        add(fx, &mut last, f.clone().into())?;
                        fx.eq.push(f);
                    }
                    if xfeed != "off" {
                        // each channel gets a low-passed, slightly delayed, quieter copy of the other one
                        let pre = ctx.create_gain()?;
                        pre.set_channel_count(2);
                        pre.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
                        pre.set_channel_interpretation(web_sys::ChannelInterpretation::Speakers);
                        add(fx, &mut last, pre.clone().into())?;
                        let split = ctx.create_channel_splitter_with_number_of_outputs(2)?;
                        let merge = ctx.create_channel_merger_with_number_of_inputs(2)?;
                        fx.nodes.push(split.clone().into());
                        fx.nodes.push(merge.clone().into());
                        pre.connect_with_audio_node(&split)?;
                        split.connect_with_audio_node_and_output_and_input(&merge, 0, 0)?;
                        split.connect_with_audio_node_and_output_and_input(&merge, 1, 1)?;
                        for (from, to) in [(0u32, 1u32), (1, 0)] {
                            let lp = ctx.create_biquad_filter()?;
                            lp.set_type(web_sys::BiquadFilterType::Lowpass);
                            lp.frequency().set_value(700.0);
                            lp.q().set_value(0.5);
                            let dl = ctx.create_delay_with_max_delay_time(0.01)?;
                            dl.delay_time().set_value(0.0003);
                            let g = ctx.create_gain()?;
                            fx.nodes.push(lp.clone().into());
                            fx.nodes.push(dl.clone().into());
                            fx.nodes.push(g.clone().into());
                            split.connect_with_audio_node_and_output(&lp, from)?;
                            lp.connect_with_audio_node(&dl)?;
                            dl.connect_with_audio_node(&g)?;
                            g.connect_with_audio_node_and_output_and_input(&merge, 0, to)?;
                            fx.cross.push(g);
                        }
                        last = merge.into();
                        let trim = ctx.create_gain()?;
                        add(fx, &mut last, trim.clone().into())?;
                        fx.trim = Some(trim);
                    }
                    if mono {
                        let m = ctx.create_gain()?;
                        m.set_channel_count(1);
                        m.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
                        m.set_channel_interpretation(web_sys::ChannelInterpretation::Speakers);
                        add(fx, &mut last, m.into())?;
                        // back to two equal channels, so the panner sees stereo and stays at unity in the middle (a mono input would
                        // take the panner's mono law: −3 dB)
                        let up = ctx.create_gain()?;
                        up.set_channel_count(2);
                        up.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
                        up.set_channel_interpretation(web_sys::ChannelInterpretation::Speakers);
                        add(fx, &mut last, up.into())?;
                    }
                    let pan = ctx.create_stereo_panner()?;
                    add(fx, &mut last, pan.clone().into())?;
                    fx.pan = Some(pan);
                }
                last.connect_with_audio_node(&ctx.destination())?;
                Ok(())
            })(&mut fx);
            if wired.is_err() { dom::console_error("Sound effects: could not build the audio graph"); }
        }
        input.gain().set_value(if on { db_gain(p.f("eq_preamp")) as f32 } else { 1.0 });
        for (f, b) in fx.eq.iter().zip(bands.iter()) {
            f.set_type(biquad_type(&b.s("type")));
            f.frequency().set_value(b.f("freq") as f32);
            f.gain().set_value(b.f("gain") as f32);
            f.q().set_value(b.f("q") as f32);
        }
        if !fx.cross.is_empty() {
            let g = db_gain(xfeed_db(&xfeed));
            for n in &fx.cross { n.gain().set_value(g as f32); }
            if let Some(t) = &fx.trim { t.gain().set_value((1.0 / (1.0 + g)) as f32); }
        }
        if let Some(pan) = &fx.pan { pan.pan().set_value((p.f("balance") / 100.0) as f32); }
    });
}
