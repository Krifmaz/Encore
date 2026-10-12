pub mod admin;
pub mod auth;
pub mod library;
pub mod picker;
pub mod playlists;
pub mod remote;
pub mod requests;
pub mod soulseek;
pub mod stats;
pub mod system;
pub mod tagging;
pub mod discover;
pub mod foryou;
pub mod catalog;

/// Text into HTML: the five characters that matter.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"), '<' => out.push_str("&lt;"), '>' => out.push_str("&gt;"), '"' => out.push_str("&quot;"), '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
