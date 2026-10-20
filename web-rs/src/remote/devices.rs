//! The Devices window: every other device (and Roon zone) with what it plays and its controls, Link / Unlink,
//! Play here and Send my music there.

use super::*;

/// One device's card in the Devices window (the body of the JS `devices.map`).
fn device_card(d: &Value, devices: &[Value], l: Option<&player::Link>, me: &str) -> String {
    let s = d.o("state");
    let name = d.s("name");
    let is_linked = l.map(|l| l.id == d.s("id")).unwrap_or(false);
    let remote_of_me = s.s("remote_of").as_str() == me;
    let now = if s.truthy("remote_of") {
        let who = if s.s("remote_of").as_str() == me { "this device".to_string() } else {
            devices.iter().find(|x| x.s("id") == s.s("remote_of")).map(|x| x.s("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "another device".into())
        };
        format!(r#"<div class="small muted">🔗 Remote for {}</div>"#, esc(&who))
    } else if s.truthy("title") {
        let label = s.g("signal").s("label");
        format!(r#"<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">{}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">{}</div><div class="small muted ellipsis">{}{}</div>
            <div class="small faint">{} · {} / {}{}</div></div></div>"#,
            art(&s.s("cover"), &if s.truthy("album") { s.s("album") } else { s.s("title") }, ""),
            esc(&s.s("title")), esc(&s.s("artist")), if s.truthy("album") { format!(" · {}", esc(&s.s("album"))) } else { String::new() },
            if s.truthy("playing") { "Playing" } else { "Paused" }, fmt::dur(s.f("position")), fmt::dur(s.f("duration")),
            if label.is_empty() { String::new() } else { format!(" · {}", esc(&label)) })
    } else {
        r#"<div class="small muted">Nothing playing</div>"#.to_string()
    };
    let link_btn = if remote_of_me || s.truthy("remote_of") { String::new() }
        else if is_linked { format!(r#"<button class="btn sm primary" data-a="unlink">{}Linked · Unlink</button>"#, icon("link")) }
        else { format!(r#"<button class="btn sm" data-a="link" title="Lock this device as a remote for {}: everything you play here plays there">{}Link as remote</button>"#, esc(&name), icon("link")) };
    let controls = if s.truthy("remote_of") { String::new() } else {
        let transport = if s.truthy("title") {
            format!(r#"<button class="btn sm ghost icon" data-a="prev" aria-label="Previous">{}</button>
            <button class="btn sm" data-a="toggle">{}</button>
            <button class="btn sm ghost icon" data-a="next" aria-label="Next">{}</button>
            <input type="range" min="0" max="1" step="0.05" value="{}" data-vol title="Volume" style="width:110px;accent-color:var(--accent)">
            {}"#,
                icon("prev"),
                if s.truthy("playing") { format!("{}Pause", icon("pause")) } else { format!("{}Play", icon("play")) },
                icon("next"),
                if s.is_nullish("volume") { "0.9".to_string() } else { s.s("volume") },
                if is_linked { "" } else { r#"<button class="btn sm" data-a="here" title="Continue this song on this device">Play here</button>"# })
        } else { String::new() };
        let send_btn = if player::current().is_some() && l.is_none() {
            format!(r#"<button class="btn sm primary" data-a="send" title="Move what you're playing to {}">Send my music there</button>"#, esc(&name))
        } else { String::new() };
        format!(r#"<div class="row wrap" style="gap:8px">
          {transport}
          {send_btn}
        </div>"#)
    };
    format!(r#"<div class="card stack {}" data-id="{}" style="gap:10px">
        <div class="row between"><b>{}{}</b>
          {link_btn}</div>
        {now}
        {controls}</div>"#,
        if is_linked { "linked" } else { "" }, esc(&d.s("id")),
        ui::icon_attr("devices", r#"width="16" style="vertical-align:-3px;margin-right:6px""#), esc(&name))
}

/// One Roon zone in the Devices window (the JS `zoneCard`).
fn zone_card(z: &Value) -> String {
    let np = z.g("now_playing");
    let vol = z.a("outputs").into_iter().find(|o| o.g("volume").is_object()).map(|o| o.o("volume"));
    let v = vol.map(|vol| { let (min, max) = (vol.f("min"), vol.f("max")); let span = if max - min == 0.0 { 1.0 } else { max - min }; ((vol.f("value") - min) / span).clamp(0.0, 1.0) });
    let state = z.s("state");
    let now = if np.is_object() {
        let img = if np.truthy("image_key") { format!("/api/roon/image/{}?s=104", String::from(js_sys::encode_uri_component(&np.s("image_key")))) } else { String::new() };
        format!(r#"<div class="row" style="gap:12px">
          <div class="thumb" style="width:52px;height:52px">{}</div>
          <div class="grow"><div class="ellipsis" style="font-weight:650">{}</div><div class="small muted ellipsis">{}{}</div>
            <div class="small faint">{}{}</div></div></div>"#,
            art(&img, &np.s("line1"), ""), esc(&np.s("line1")), esc(&np.s("line2")), if np.truthy("line3") { format!(" · {}", esc(&np.s("line3"))) } else { String::new() },
            match state.as_str() { "playing" => "Playing", "paused" => "Paused", "loading" => "Loading", _ => "Stopped" },
            if np.truthy("length") { format!(" · {} / {}", fmt::dur(np.f("seek_position")), fmt::dur(np.f("length"))) } else { String::new() })
    } else { r#"<div class="small muted">Nothing playing</div>"#.to_string() };
    format!(r#"<div class="card stack" data-zone="{}" style="gap:10px">
        <div class="row between"><b>{}{} <span class="badge" style="margin-left:6px">Roon</span></b>
          <button class="btn sm" data-z="search" title="Search Roon's library and play the first match here">Play in Roon…</button></div>
        {now}
        <div class="row wrap" style="gap:8px">
          <button class="btn sm ghost icon" data-z="prev" aria-label="Previous" {}>{}</button>
          <button class="btn sm" data-z="toggle">{}</button>
          <button class="btn sm ghost icon" data-z="next" aria-label="Next" {}>{}</button>
          {}
        </div></div>"#,
        esc(&z.s("zone_id")), ui::icon_attr("devices", r#"width="16" style="vertical-align:-3px;margin-right:6px""#), esc(&z.s("name")),
        if z.truthy("is_previous_allowed") { "" } else { "disabled" }, icon("prev"),
        if state == "playing" { format!("{}Pause", icon("pause")) } else { format!("{}Play", icon("play")) },
        if z.truthy("is_next_allowed") { "" } else { "disabled" }, icon("next"),
        v.map(|v| format!(r#"<input type="range" min="0" max="1" step="0.02" value="{}" data-zvol title="Volume" style="width:110px;accent-color:var(--accent)">"#, v)).unwrap_or_default())
}

/// The Devices window: what every other device is playing, with controls, and Link / Unlink.
pub fn open() {
    let m = ui::modal(ModalOpts::new("Your devices", r#"<div id="dev-me" class="stack" style="margin-bottom:12px"></div><div id="dev-list" class="stack"><div class="skeleton" style="height:90px"></div></div>"#));
    let (Some(list), Some(me_box)) = (m.qs("#dev-list"), m.qs("#dev-me")) else { return };
    let timer: Rc<RefCell<Option<dom::Interval>>> = Rc::new(RefCell::new(None));
    // Roon zones (when Roon is enabled): shown before the Encore devices, refreshed with them
    let roon_on = app::has_feature("roon");
    let zones: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));

    // paint(): the list from DEVICES (what the server last pushed or we last fetched)
    let paint: Rc<dyn Fn()> = {
        let (list, timer, zones) = (list.clone(), timer.clone(), zones.clone());
        Rc::new(move || {
            if !list.is_connected() { timer.borrow_mut().take(); ON_DEVICES.with(|o| *o.borrow_mut() = None); return; }
            let my = me();
            let devs = DEVICES.with(|d| d.borrow().clone());
            let zs = zones.borrow();
            if devs.is_empty() && zs.is_empty() {
                dom::set_html(&list, &format!(r#"<p class="muted" style="margin:0">No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.{}</p>"#,
                    if roon_on { " Roon zones appear once Encore is connected to your Roon Core (Settings → Integrations → Roon)." } else { "" }));
                return;
            }
            let l = player::linked();
            dom::set_html(&list, &format!("{}{}", zs.iter().map(zone_card).collect::<String>(), devs.iter().map(|d| device_card(d, &devs, l.as_ref(), &my)).collect::<String>()));
        })
    };
    let refresh_zones: Rc<dyn Fn()> = {
        let (list, zones, paint) = (list.clone(), zones.clone(), paint.clone());
        Rc::new(move || {
            if !roon_on || !list.is_connected() { return; }
            let (zones, paint) = (zones.clone(), paint.clone());
            dom::spawn(async move {
                // Roon away: keep what we had
                if let Ok(z) = api::get("/roon/zones").await {
                    let items = z.items();
                    if serde_json::to_string(&items).ok() != serde_json::to_string(&*zones.borrow()).ok() { *zones.borrow_mut() = items; paint(); }
                }
            });
        })
    };
    let refresh: Rc<dyn Fn()> = {
        let (list, timer, paint, refresh_zones) = (list.clone(), timer.clone(), paint.clone(), refresh_zones.clone());
        Rc::new(move || {
            let (list, timer, paint, refresh_zones) = (list.clone(), timer.clone(), paint.clone(), refresh_zones.clone());
            dom::spawn(async move {
                if !list.is_connected() { timer.borrow_mut().take(); ON_DEVICES.with(|o| *o.borrow_mut() = None); return; }
                match api::get("/remote/devices").await {
                    Ok(v) => got_devices(v.items()),
                    Err(_) => { DEVICES.with(|d| d.borrow_mut().clear()); paint(); }
                }
                refresh_zones();
            });
        })
    };
    {
        let (zones, refresh_zones) = (zones.clone(), refresh_zones.clone());
        dom::on(&list, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-z]") else { return };
            let id = dom::closest_of(&b, "[data-zone]").map(|c| dom::data(&c, "zone")).unwrap_or_default();
            let name = zones.borrow().iter().find(|x| x.s("zone_id") == id).map(|z| z.s("name"));
            let what = dom::data(&b, "z");
            let refresh_zones = refresh_zones.clone();
            dom::spawn(async move {
                let r = if what == "search" {
                    let Some(q) = dom::prompt(&format!("Play in Roon ({}): artist, album or song", name.clone().unwrap_or_else(|| "zone".into())), "") else { return };
                    if q.trim().is_empty() { return; }
                    match api::post(&format!("/roon/zones/{}/play", String::from(js_sys::encode_uri_component(&id))), json!({ "query": q.trim() })).await {
                        Ok(r) => { toast(&format!("Playing {} in {}", r.s("playing"), name.unwrap_or_else(|| "Roon".into())), "ok"); Ok(()) }
                        Err(e) => Err(e),
                    }
                } else {
                    api::post(&format!("/roon/zones/{}/command", String::from(js_sys::encode_uri_component(&id))), json!({ "action": what })).await.map(|_| ())
                };
                match r {
                    Ok(()) => dom::timeout(500, move || refresh_zones()),
                    Err(e) => toast(&e.message, "err"),
                }
            });
        });
        dom::on(&list, "change", move |e| {
            let Some(v) = dom::closest(e, "[data-zvol]") else { return };
            let id = dom::closest_of(&v, "[data-zone]").map(|c| dom::data(&c, "zone")).unwrap_or_default();
            let n: f64 = num(&dom::value(&v));
            dom::spawn(async move {
                if let Err(e) = api::post(&format!("/roon/zones/{}/command", String::from(js_sys::encode_uri_component(&id))), json!({ "action": "volume", "value": n })).await { toast(&e.message, "err"); }
            });
        });
    }

    let send: Rc<dyn Fn(String, Value)> = {
        let refresh = refresh.clone();
        Rc::new(move |id, body| {
            let refresh = refresh.clone();
            dom::spawn(async move {
                match send_to(id, body).await {
                    Ok(_) => { if !streaming() { dom::timeout(400, move || refresh()); } }
                    Err(e) => toast(&e.message, "err"),
                }
            });
        })
    };

    // paintMe calls itself from its own buttons
    type PaintMe = Rc<RefCell<Option<Rc<dyn Fn()>>>>;
    let paint_me_cell: PaintMe = Rc::new(RefCell::new(None));
    let paint_me: Rc<dyn Fn()> = {
        let (me_box, paint, cell) = (me_box.clone(), paint.clone(), paint_me_cell.clone());
        Rc::new(move || {
            let l = player::linked();
            dom::set_html(&me_box, &format!(r##"<div class="row wrap between" style="gap:8px"><div class="small muted">This device: <b style="color:var(--text)">{}</b> <a href="#" id="dev-rename" class="small">rename</a></div>
      {}</div>
      {}"##,
                esc(&device_name()),
                l.as_ref().map(|l| format!(r#"<button class="btn sm" id="dev-unlink">{}Unlink from {}</button>"#, icon("link"), esc(&l.name))).unwrap_or_default(),
                l.as_ref().map(|l| format!(r#"<div class="small muted">🔗 Linked: everything you play here goes to <b>{}</b>. Unlink to listen on this device.</div>"#, esc(&l.name))).unwrap_or_default()));
            let again = { let cell = cell.clone(); move || { let f = cell.borrow().clone(); if let Some(f) = f { f() } } };
            if let Some(b) = dom::qs_in(&me_box, "#dev-unlink") {
                let (again, paint) = (again.clone(), paint.clone());
                dom::on(&b, "click", move |_| { unlink(); again(); paint(); });
            }
            if let Some(a) = dom::qs_in(&me_box, "#dev-rename") {
                dom::on(&a, "click", move |e| {
                    dom::prevent(e);
                    let Some(n) = dom::prompt("Name this device (shown on your other devices)", &device_name()) else { return };
                    let t = js_sys::JsString::from(n.as_str()).trim();
                    if t.length() > 0 { dom::ls_set(NAME_KEY, &String::from(t.slice(0, 60))); } else { dom::ls_remove(NAME_KEY); }
                    again();
                    dom::spawn(beat());
                });
            }
        })
    };
    *paint_me_cell.borrow_mut() = Some(paint_me.clone());

    {
        let (send, paint_me, paint, m) = (send.clone(), paint_me.clone(), paint.clone(), m.clone());
        dom::on(&list, "click", move |e| {
            let Some(b) = dom::closest(e, "[data-a]") else { return };
            let id = dom::closest_of(&b, "[data-id]").map(|c| dom::data(&c, "id")).unwrap_or_default();
            let d = DEVICES.with(|d| d.borrow().iter().find(|d| d.s("id") == id).cloned());
            let Some(d) = d else { return };
            let a = dom::data(&b, "a");
            match a.as_str() {
                "link" => { link_to(d); paint_me(); paint(); }
                "unlink" => { unlink(); paint_me(); paint(); }
                "here" => {
                    let s = d.o("state");
                    let queue = s.a("queue");
                    if queue.is_empty() { return; }
                    if player::linked().is_some() { unlink(); }
                    player::play_queue(queue, s.i("index").max(0) as usize, "list");
                    let at = s.f("position");
                    dom::timeout(600, move || player::seek_to(at));
                    send(d.s("id"), json!({ "action": "pause" }));
                    m.close();
                }
                "send" => {
                    let snap = player::snapshot();
                    let queue = snap.a("queue");
                    if queue.is_empty() { toast("Only songs from your library can be sent", "err"); return; }
                    let index = queue.iter().position(|t| t.g("id") == snap.g("id")).map(|i| i as i64).unwrap_or(-1);
                    send(d.s("id"), json!({ "action": "play_queue", "tracks": queue, "index": index, "position": snap.g("position") }));
                    player::command(json!({ "action": "pause" }));
                }
                _ => send(d.s("id"), json!({ "action": a })),
            }
        });
    }
    {
        let send = send.clone();
        dom::on(&list, "change", move |e| {
            let Some(v) = dom::closest(e, "[data-vol]") else { return };
            let id = dom::closest_of(&v, "[data-id]").map(|c| dom::data(&c, "id")).unwrap_or_default();
            let n: f64 = num(&dom::value(&v));
            let value = if n.fract() == 0.0 { json!(n as i64) } else { json!(n) };
            send(id, json!({ "action": "volume", "value": value }));
        });
    }
    ON_DEVICES.with(|o| *o.borrow_mut() = Some(paint.clone()));
    paint_me();
    refresh();
    // pushed updates repaint the list; this is the fallback when the stream is down (Roon zones are always polled)
    let (r, rz) = (refresh.clone(), refresh_zones.clone());
    *timer.borrow_mut() = Some(dom::Interval::new(2000, move || { if !streaming() { r(); } else { rz(); } }));
}
