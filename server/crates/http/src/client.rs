//! The HTTP client every service call goes through.
use std::time::Duration;

/// `Encore/<version> ( <contact> )`: MetaBrainz (ListenBrainz, MusicBrainz) turn away clients without a
/// way to contact them. The contact is the `contact_email` setting, else the project's page.
pub fn user_agent(contact: &str) -> String {
    format!("Encore/{} ( {} )", encore_core::VERSION, if contact.trim().is_empty() { "https://github.com/Krifmaz/Encore".to_string() } else { contact.to_string() })
}

/// The server's client: its User-Agent and a 30 s timeout for every request that sets none of its own.
pub fn build(contact: &str) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().user_agent(user_agent(contact)).timeout(Duration::from_secs(30)).build()
}

/// A client that hands redirects back instead of following them (60 s per request): an indexer's download
/// link often redirects to a magnet link, which no HTTP client can follow, so the caller follows them.
pub fn no_redirects(contact: &str) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().user_agent(user_agent(contact)).redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(60)).build()
}

/// A client for a program on this machine (the translator's model server on 127.0.0.1): never through a
/// proxy, and no timeout of its own (an answer takes as long as the model needs).
pub fn local() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().no_proxy().build()
}

#[cfg(test)]
mod tests {
    #[test]
    fn user_agent_names_a_contact() {
        assert!(super::user_agent("").ends_with("( https://github.com/Krifmaz/Encore )"));
        assert!(super::user_agent("  ").ends_with("( https://github.com/Krifmaz/Encore )"));
        assert!(super::user_agent("me@example.org").starts_with("Encore/"));
        assert!(super::user_agent("me@example.org").ends_with("( me@example.org )"));
    }
}
