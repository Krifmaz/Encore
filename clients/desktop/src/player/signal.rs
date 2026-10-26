//! Signal path: what the file is, what Encore sends to this computer and what Cadenza does
//! with it before the output – the web's Signal path window with the engine's own stages.

use super::{devices::Linked, Message, State};
use crate::api::{Api, ApiError};
use crate::engine::Status;
use crate::fmt;
use crate::pages::{Ctx, El};
use crate::theme;
use crate::widgets::{self, t};
use iced::widget::{column, container};
use iced::{Length, Task};
use serde_json::Value;
use std::sync::Arc;

/// The `/info` of the song the dialog is about.
#[derive(Debug, Default)]
pub struct Info {
    pub id: Option<i64>,
    pub info: Option<Value>,
    pub loading: bool,
}

impl Info {
    /// Ask the server about `id` (nothing when it is already here).
    pub fn load(&mut self, api: Arc<Api>, id: i64) -> Task<Message> {
        if self.id == Some(id) && (self.info.is_some() || self.loading) {
            return Task::none();
        }
        self.id = Some(id);
        self.info = None;
        self.loading = true;
        Task::perform(
            async move { api.get(&format!("/library/tracks/{id}/info")).await },
            move |r| Message::TrackInfo(id, r),
        )
    }

    pub fn loaded(&mut self, id: i64, r: Result<Value, ApiError>) {
        if self.id != Some(id) {
            return;
        }
        self.loading = false;
        self.info = r.ok();
    }
}

/// `codec · 24-bit / 96 kHz · stereo · 1411 kbps` from `/info` (or the row's format).
pub fn source_line(format: &str, bitrate: f64, info: Option<&Value>) -> String {
    let Some(info) = info else {
        return format!(
            "{}{}",
            format.to_ascii_uppercase(),
            if bitrate > 0.0 { format!(" · {} kbps", bitrate.round() as i64) } else { String::new() }
        );
    };
    let codec = fmt::s(info, "codec");
    let mut parts = vec![if codec.is_empty() { format.to_string() } else { codec }.to_ascii_uppercase()];
    let (depth, rate) = (fmt::i(info, "bit_depth"), fmt::f(info, "sample_rate"));
    if fmt::truthy(info, "lossless") && depth > 0 && rate > 0.0 {
        parts.push(format!("{depth}-bit / {} kHz", khz(rate)));
    } else if rate > 0.0 {
        parts.push(format!("{} kHz", khz(rate)));
    }
    let ch = fmt::i(info, "channels");
    if ch > 0 {
        parts.push(match ch {
            2 => "stereo".into(),
            1 => "mono".into(),
            n => format!("{n} channels"),
        });
    }
    let br = fmt::f(info, "bitrate");
    if br > 0.0 {
        parts.push(format!("{} kbps", br.round() as i64));
    }
    parts.join(" · ")
}

/// `44100 → 44.1`, `48000 → 48`, `22050 → 22.05`, `11025 → 11.025`: exact for every common rate (up to three
/// decimals, no trailing zeros).
pub fn khz(hz: f64) -> String {
    let s = format!("{:.3}", hz / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The words of the quality badge: the engine's path label while it plays (`FLAC 16/44.1 →
/// 24/176.4`, `FLAC 16/44.1 · bit-perfect`), the song's format before, the linked device's label
/// when linked.
pub fn quality_label(st: &Status, linked: Option<&Linked>) -> String {
    if let Some(l) = linked {
        let label = fmt::s(&l.state["signal"], "label");
        if !label.is_empty() {
            return label;
        }
        return fmt::s(&l.state, "format").to_ascii_uppercase();
    }
    if let Some(p) = st.signal_path.as_ref() {
        if !p.label.is_empty() {
            return p.label.clone();
        }
    }
    st.track
        .as_ref()
        .and_then(|t| t.format.clone())
        .or_else(|| st.source_format.clone())
        .unwrap_or_default()
}

/// One row of the dialog: `(name, value, note)`.
pub type Row = (String, String, String);
/// The dialog's verdict: `(head, text)`.
pub type Verdict = (String, &'static str);

/// The dialog's rows and its verdict.
pub fn rows(st: &Status, info: Option<&Value>, linked: Option<&Linked>) -> (Vec<Row>, Verdict) {
    let mut rows: Vec<Row> = Vec::new();
    let (format, bitrate) = match linked {
        Some(l) => (fmt::s(&l.state, "format"), 0.0),
        None => (
            st.track.as_ref().and_then(|t| t.format.clone()).unwrap_or_default(),
            0.0,
        ),
    };
    let suspect = info
        .map(|i| &i["quality"])
        .filter(|q| fmt::s(q, "status") == "suspect")
        .map(|q| fmt::s(q, "summary"))
        .unwrap_or_default();
    let lossy = if info.is_some_and(|i| i["lossless"] == Value::Bool(false)) {
        "A lossy file: what the file lost is gone before Encore touches it".to_string()
    } else {
        String::new()
    };
    rows.push((
        "Source".into(),
        source_line(&format, bitrate, info),
        [lossy, suspect, crate::pages::own_file::own_file_note(info)]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
    ));
    if let Some(l) = linked {
        let sig = &l.state["signal"];
        let eng = &l.state["engine"];
        let tr = fmt::s(sig, "transport");
        let what = if tr == "original" {
            "The original file, byte for byte".to_string()
        } else if tr.is_empty() {
            "Not reported by that device".to_string()
        } else if tr == "flac" {
            "Converted to FLAC by the server (lossless)".to_string()
        } else if let Some(rate) = tr.strip_prefix("cadenza-") {
            format!(
                "Upsampled on the server by Cadenza ({} kHz)",
                khz(rate.parse().unwrap_or(0.0))
            )
        } else if tr.starts_with("aac-") || tr.starts_with("opus-") {
            let (c, k) = tr.split_once('-').unwrap_or((&tr, ""));
            format!("{} {k} kbps (lossy)", c.to_ascii_uppercase())
        } else {
            tr.clone()
        };
        rows.push((format!("Encore → {}", l.name), what, String::new()));
        let stages = fmt::items(&sig["stages"]);
        if !stages.is_empty() {
            for s in &stages {
                rows.push((fmt::s(s, "name"), fmt::s(s, "detail"), String::new()));
            }
            rows.push((
                "Output".into(),
                format!(
                    "{}{}{}",
                    fmt::s(eng, "output"),
                    if fmt::truthy(eng, "exclusive") { " · exclusive" } else { "" },
                    if fmt::truthy(sig, "bitperfect") { " · bit-perfect" } else { "" }
                ),
                String::new(),
            ));
            return (
                rows,
                (
                    format!("Played by Cadenza on {}", l.name),
                    "Every stage above is what the engine reports. Bit-perfect means nothing touched the samples.",
                ),
            );
        }
        let out_rate = fmt::f(sig, "output_rate");
        rows.push((
            l.name.clone(),
            if out_rate > 0.0 {
                format!("Plays through its browser audio at {} kHz", khz(out_rate))
            } else {
                "Playing there".into()
            },
            String::new(),
        ));
        return (
            rows,
            (
                "Playing on another device".into(),
                "The rows above are what that device reports. Open Encore there for its own signal path.",
            ),
        );
    }
    rows.push((
        "Encore → this computer".into(),
        "The original file, byte for byte – nothing was converted or resampled on the way".into(),
        String::new(),
    ));
    if let Some(r) = norm_row(st, info, crate::engine::normalize::prefs()) {
        rows.push(r);
    }
    let engine_rows = crate::engine::settings::signal_rows(st);
    let bit_perfect = st.signal_path.as_ref().is_some_and(|p| p.bit_perfect);
    if engine_rows.is_empty() {
        rows.push((
            "This computer".into(),
            format!("Cadenza · {}", crate::engine::settings::output_line(st).trim_start_matches("Output: ")),
            if st.track.is_some() {
                "The engine reports its stages once the song plays.".into()
            } else {
                String::new()
            },
        ));
    } else {
        for (k, v) in engine_rows {
            rows.push((k, v, String::new()));
        }
    }
    let verdict = if bit_perfect {
        (
            "Pure playback from Encore".to_string(),
            "Encore sent the file exactly as it is stored and Cadenza handed it to the device untouched. Bit-perfect means nothing touched the samples.",
        )
    } else {
        (
            "Played by Cadenza on this computer".to_string(),
            "Every stage above is what the engine reports. Bit-perfect means nothing touched the samples.",
        )
    };
    (rows, verdict)
}

/// The "Normalization" row while normalization is on: the gain this song plays with (the row's
/// `gain`, else the one `/info` reports), why, and where the numbers come from.
pub fn norm_row(st: &Status, info: Option<&Value>, prefs: crate::engine::normalize::Prefs) -> Option<Row> {
    use crate::engine::normalize::{self, Gain};
    if !prefs.on() {
        return None;
    }
    let known = st
        .track
        .as_ref()
        .and_then(crate::engine::tracks::encore_id)
        .and_then(normalize::gain_of)
        .or_else(|| info.and_then(Gain::from_row));
    let d = normalize::decide(prefs, known.as_ref(), st.settings.dsp.bit_perfect);
    let source = info.map(|i| fmt::s(&i["gain"], "source")).unwrap_or_default();
    Some(("Normalization".into(), d.line(), d.note(&source)))
}

/// The dialog's body.
pub fn view<'a>(state: &'a State, _ctx: &Ctx<'a>) -> El<'a, Message> {
    if state.signal.loading && state.signal.info.is_none() && state.devices.linked.is_none() {
        return widgets::skeleton_rows(3);
    }
    let (rows, (head, text)) = rows(&state.status, state.signal.info.as_ref(), state.devices.linked.as_ref());
    let mut col = column![].spacing(12).width(Length::Fill);
    for (k, v, note) in rows {
        let mut item = column![widgets::small_muted(k), t(v).size(14).font(theme::semibold())].spacing(2);
        if !note.is_empty() {
            item = item.push(widgets::small_muted(note));
        }
        col = col.push(item);
    }
    col = col.push(
        container(column![t(head).size(14).font(theme::bold()), widgets::small_muted(text)].spacing(4))
            .padding(12)
            .width(Length::Fill)
            .style(theme::card),
    );
    col.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Track;
    use serde_json::json;

    #[test]
    fn source_and_quality_words() {
        assert_eq!(source_line("flac", 0.0, None), "FLAC");
        assert_eq!(source_line("mp3", 320.0, None), "MP3 · 320 kbps");
        let info = json!({ "codec": "flac", "lossless": true, "bit_depth": 24, "sample_rate": 96000, "channels": 2, "bitrate": 2304 });
        assert_eq!(source_line("flac", 0.0, Some(&info)), "FLAC · 24-bit / 96 kHz · stereo · 2304 kbps");
        assert_eq!(khz(44100.0), "44.1");
        assert_eq!((khz(22050.0).as_str(), khz(48000.0).as_str(), khz(11025.0).as_str(), khz(352800.0).as_str()), ("22.05", "48", "11.025", "352.8"));
        let st = Status {
            track: Some(Track {
                format: Some("FLAC".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(quality_label(&st, None), "FLAC");
        let l = Linked {
            name: "Phone".into(),
            state: json!({ "signal": { "label": "FLAC → AAC 256k", "transport": "aac-256" }, "format": "flac" }),
            ..Default::default()
        };
        assert_eq!(quality_label(&st, Some(&l)), "FLAC → AAC 256k");
        let (r, (head, _)) = rows(&st, None, Some(&l));
        assert_eq!(r[1].0, "Encore → Phone");
        assert_eq!(r[1].1, "AAC 256 kbps (lossy)");
        assert_eq!(head, "Playing on another device");
        let (r, (head, _)) = rows(&st, Some(&json!({ "lossless": false, "codec": "mp3" })), None);
        assert_eq!(r[0].2, "A lossy file: what the file lost is gone before Encore touches it");
        assert_eq!(r[1].0, "Encore → this computer");
        assert_eq!(head, "Played by Cadenza on this computer");
    }

    #[test]
    fn the_normalization_row() {
        use crate::engine::normalize::{Mode, Prefs};
        let mut st = Status::default();
        let info = json!({ "gain": { "track": -8.0, "album": -6.5, "track_peak": 0.5, "album_peak": 0.6, "source": "tags", "lufs": -10.0 } });
        let off = Prefs::default();
        assert!(norm_row(&st, Some(&info), off).is_none(), "only while normalization is on");
        let album = Prefs { mode: Mode::Album, target: -14, clip: true };
        let (name, value, note) = norm_row(&st, Some(&info), album).unwrap();
        assert_eq!(name, "Normalization");
        assert_eq!(value, "\u{2212}2.5 dB (album gain, target \u{2212}14 LUFS)");
        assert_eq!(note, "From the file's ReplayGain tags.");
        let (_, value, note) = norm_row(&st, None, album).unwrap();
        assert_eq!((value.as_str(), note.as_str()), ("No loudness data for this song", "It plays at its own level until Encore has measured it."));
        st.settings.dsp.bit_perfect = true;
        let (_, value, note) = norm_row(&st, Some(&info), album).unwrap();
        assert_eq!((value.as_str(), note.as_str()), ("Off for bit-perfect output", "The DAC gets the samples untouched."));
    }
}
