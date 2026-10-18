//! The engine's settings for Settings → Playback (package E draws the card and edits
//! `cadenza::config::Settings` itself): reading them, checking and applying a change, which
//! output is exclusive, and the status words the card and the player's Signal path dialog
//! show (`output_line`, `exclusive_failure`, `signal_rows`). A change goes through
//! `engine.update_settings` and is persisted by the embedded player; before the engine runs
//! the config file is written directly.

use super::Status;
use cadenza::config::Settings;
use cadenza_dsp::signal_path::rate_label;

fn config_path() -> std::path::PathBuf {
    crate::config::cadenza_dir().join("config.toml")
}

/// The settings in force: the engine's when it runs, else the config file's. On a computer
/// that never ran the engine the plain mode (bit-perfect) is the default: the Cadenza switch
/// is off.
pub fn settings() -> Settings {
    if let Some(e) = super::engine() {
        return e.settings();
    }
    let path = config_path();
    if !path.exists() {
        let mut s = Settings::default();
        s.dsp.bit_perfect = true;
        return s;
    }
    cadenza::config::Config::load(&path)
        .map(|c| c.settings())
        .unwrap_or_default()
}

/// Check and hand `s` to the engine (or write it to the config file before the engine runs).
pub fn apply(s: Settings) -> Result<(), String> {
    cadenza::api::validate(&s)?;
    if let Some(e) = super::engine() {
        e.update_settings(s);
        return Ok(());
    }
    let path = config_path();
    let mut c = cadenza::config::Config::load(&path).map_err(|e| e.to_string())?;
    c.apply(&s);
    c.save(&path).map_err(|e| e.to_string())
}

// ----- exclusive mode -------------------------------------------------------------------

/// The exclusive (direct) backend of this build, if any.
pub fn exclusive_backend() -> Option<&'static str> {
    cadenza::output::backends()
        .into_iter()
        .find(|b| matches!(*b, "wasapi" | "coreaudio" | "alsa" | "asio"))
}

/// Whether the settings ask for an exclusive output.
pub fn exclusive_wanted(s: &Settings) -> bool {
    let (b, _) = cadenza::output::split_backend(&s.output.backend, &s.output.device);
    !matches!(b, "cpal" | "null" | "endpoint")
}

// ----- status --------------------------------------------------------------------------

/// The name of the output as the status reports it, or as the settings name it.
fn output_name(st: &Status) -> String {
    if !st.output.is_empty() {
        return st.output.clone();
    }
    let s = &st.settings;
    let (b, d) = cadenza::output::split_backend(&s.output.backend, &s.output.device);
    if d.is_empty() {
        b.to_string()
    } else {
        format!("{b} · {d}")
    }
}

/// "Output: <device> · <format> · Exclusive | Shared", or "· not opened yet".
pub fn output_line(st: &Status) -> String {
    match st.output_format {
        Some(f) => format!(
            "Output: {} · {} · {}",
            output_name(st),
            f.label(),
            if st.output_exclusive { "Exclusive" } else { "Shared" }
        ),
        None => format!("Output: {} · not opened yet", output_name(st)),
    }
}

/// "Exclusive mode failed: <reason>" when exclusive output was asked for and the engine
/// reports an error instead (the reason names the holding app when the driver says).
pub fn exclusive_failure(st: &Status) -> Option<String> {
    let e = st.error.as_ref()?;
    if exclusive_wanted(&st.settings) && !st.output_exclusive {
        Some(format!("Exclusive mode failed: {e}"))
    } else {
        None
    }
}

/// The engine's signal path as `(name, detail)` rows: the input, every active stage, the output.
pub fn signal_rows(st: &Status) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let Some(p) = st.signal_path.as_ref() else {
        return rows;
    };
    rows.push((
        "Input".into(),
        format!(
            "{} {}-bit / {} kHz · {}",
            p.input.format,
            p.input.bits,
            rate_label(p.input.rate),
            channels(p.input.channels)
        ),
    ));
    for s in p.stages.iter().filter(|s| s.active) {
        rows.push((s.name.clone(), s.detail.clone()));
    }
    let mut out = match st.output_format {
        Some(f) => format!("{} · {}", output_name(st), f.label()),
        None => format!(
            "{} · {}-bit / {} kHz",
            output_name(st),
            p.output.bits,
            rate_label(p.output.rate)
        ),
    };
    out.push_str(if st.output_exclusive { " · exclusive" } else { " · shared" });
    if p.bit_perfect {
        out.push_str(" · bit-perfect");
    }
    rows.push(("Output".into(), out));
    rows
}

fn channels(n: u32) -> String {
    match n {
        1 => "mono".into(),
        2 => "stereo".into(),
        n => format!("{n} channels"),
    }
}

/// Play the two-second test tone through the engine.
pub fn test_sound() -> Result<(), String> {
    super::test_sound::play()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines() {
        let mut s = Settings::default();
        s.output.backend = "alsa".into();
        s.output.device = "hw:1,0".into();
        assert!(exclusive_wanted(&s));
        let mut st = Status {
            settings: s.clone(),
            ..Default::default()
        };
        assert_eq!(output_line(&st), "Output: alsa · hw:1,0 · not opened yet");
        st.output = "alsa · hw:1,0".into();
        st.output_format = Some(cadenza::output::OutputFormat::pcm(176_400, 24, 2));
        st.output_exclusive = true;
        assert_eq!(output_line(&st), "Output: alsa · hw:1,0 · 24-bit 176.4 kHz · Exclusive");
        assert!(exclusive_failure(&st).is_none());
        st.output_exclusive = false;
        st.error = Some("device busy: Spotify".into());
        assert_eq!(exclusive_failure(&st).as_deref(), Some("Exclusive mode failed: device busy: Spotify"));
        assert!(signal_rows(&st).is_empty(), "no path while stopped");
        s.output.backend = "cpal".into();
        assert!(!exclusive_wanted(&s));
    }
}
