//! Android: the media notification / lock-screen controls come from the Kotlin plugin (added with the
//! Android build); until then nowPlaying() is remembered only.

use crate::bridge::NowPlaying;
use tauri::{AppHandle, Runtime};

pub fn now_playing<R: Runtime>(_app: &AppHandle<R>, _np: NowPlaying) {}
