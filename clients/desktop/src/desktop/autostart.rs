//! Start Encore when the user signs in to the computer (Settings → This computer): an XDG
//! autostart entry on Linux, a launch agent on macOS, the Run key of the current user on
//! Windows. The app starts with `--minimized`, in the tray. The config's `autostart` is what the
//! user chose; the system's entry is brought in line with it.

use super::guarded;
use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use std::sync::Mutex;

/// The flag the system starts the app with.
pub const MINIMIZED_ARG: &str = "--minimized";

/// The entry's name. Not plain "Encore": the classic app's entry keeps its own name, so turning
/// one off never removes the other's.
pub const ENTRY: &str = "Encore Desktop";

static APPLIED: Mutex<Option<bool>> = Mutex::new(None);

/// What the system should start: the AppImage itself when running from one (its mount point
/// changes every start), else this program.
pub fn app_path() -> Result<String, String> {
    if let Some(a) = std::env::var_os("APPIMAGE").filter(|a| std::path::Path::new(a).is_file()) {
        return Ok(a.to_string_lossy().into_owned());
    }
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

/// The program as the entry names it. auto-launch writes `Exec=<path> <args>` (Linux) and
/// `<path> <args>` (the Windows Run value) as they are, so a path with a space would be cut at
/// the space: it is quoted there. On Linux the Desktop Entry rules apply: inside the quotes `"`,
/// `` ` ``, `$` and `\` get a backslash, doubled because the value is unescaped once as a string
/// first, and `%` becomes `%%`. macOS gets a list of arguments, unquoted.
fn entry_path(path: &str) -> String {
    if cfg!(target_os = "linux") {
        let mut q = String::with_capacity(path.len() + 2);
        q.push('"');
        for c in path.chars() {
            match c {
                // the string-value escape and the Exec-quoting escape, one after the other
                '\\' => q.push_str("\\\\\\\\"),
                '"' | '`' | '$' => {
                    q.push_str("\\\\");
                    q.push(c);
                }
                '%' => q.push_str("%%"),
                c => q.push(c),
            }
        }
        q.push('"');
        q
    } else if cfg!(windows) {
        // Windows paths cannot hold a double quote.
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

fn launcher() -> Result<AutoLaunch, String> {
    let path = entry_path(&app_path()?);
    let mut b = AutoLaunchBuilder::new();
    b.set_app_name(ENTRY)
        .set_app_path(&path)
        .set_args(&[MINIMIZED_ARG]);
    #[cfg(target_os = "macos")]
    b.set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent);
    #[cfg(target_os = "windows")]
    b.set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser);
    b.build().map_err(|e| e.to_string())
}

/// Whether the system starts Encore now.
pub fn is_enabled() -> bool {
    guarded("Start with the computer", || {
        launcher().and_then(|l| l.is_enabled().map_err(|e| e.to_string()))
    })
    .and_then(Result::ok)
    .unwrap_or(false)
}

/// Turn it on or off; the error is in the classic app's words.
pub fn set(on: bool) -> Result<(), String> {
    let r = guarded("Start with the computer", || {
        let l = launcher()?;
        if on {
            l.enable().map_err(|e| e.to_string())
        } else if l.is_enabled().unwrap_or(false) {
            l.disable().map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    })
    .unwrap_or_else(|| Err("not available here".into()));
    r.map_err(|e| format!("Couldn't change start with the computer: {e}"))
}

/// The choice as last applied (None before the first time).
pub fn applied() -> Option<bool> {
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bring the system in line with `want` (on: the entry is written again, so it points at this
/// copy of the app). Returns the error to show, if any. Tried once per choice.
pub fn reconcile(want: bool) -> Option<String> {
    *APPLIED.lock().unwrap_or_else(|e| e.into_inner()) = Some(want);
    let r = if want || is_enabled() {
        set(want)
    } else {
        Ok(())
    };
    match r {
        Ok(()) => {
            log::info!(
                "start with the computer: {}",
                if want { "on" } else { "off" }
            );
            None
        }
        Err(e) => {
            log::warn!("{e}");
            Some(e)
        }
    }
}

/// Whether this process was started by the system at sign-in.
pub fn started_minimized() -> bool {
    std::env::args().any(|a| a == MINIMIZED_ARG)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_with_spaces_are_quoted_for_the_entry() {
        let p = entry_path("/home/me/Apps/Encore 0.27.AppImage");
        if cfg!(target_os = "linux") {
            assert_eq!(p, "\"/home/me/Apps/Encore 0.27.AppImage\"");
            assert_eq!(entry_path("/a/50% \"x\"$"), r#""/a/50%% \\"x\\"\\$""#);
        } else if cfg!(windows) {
            assert_eq!(p, "\"/home/me/Apps/Encore 0.27.AppImage\"");
        } else {
            assert_eq!(p, "/home/me/Apps/Encore 0.27.AppImage");
        }
    }

    #[test]
    fn the_entry_points_at_this_program() {
        let p = app_path().unwrap();
        assert!(!p.is_empty());
        assert_ne!(ENTRY, "Encore", "the classic app's entry stays its own");
        assert!(!started_minimized());
    }
}
