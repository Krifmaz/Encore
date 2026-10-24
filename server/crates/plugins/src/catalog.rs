//! The catalog: the plugins Encore ships (first-party), embedded in the binary. A fresh install shows each one
//! as "available, not installed"; an upgrade adopts and installs the ones that were in use
//! ([`crate::Manifest::in_use`]). The manifests are `catalog/<id>.toml`; the catalog test checks every one.
use crate::manifest::{parse_for, Manifest, Origin};
use std::sync::OnceLock;

/// Every catalog manifest as (id, text), in the order the Plugins page lists them: the sources, the download
/// clients, the tools and playback, the connections (the Apple Music wrapper sits beside gamdl, which it serves). The text is what the plugins table keeps.
pub const FILES: [(&str, &str); 20] = [
    ("ytdlp", include_str!("../catalog/ytdlp.toml")),
    ("streamrip", include_str!("../catalog/streamrip.toml")),
    ("slskd", include_str!("../catalog/slskd.toml")),
    ("zotify", include_str!("../catalog/zotify.toml")),
    ("gamdl", include_str!("../catalog/gamdl.toml")),
    ("wrapper", include_str!("../catalog/wrapper.toml")),
    ("torznab", include_str!("../catalog/torznab.toml")),
    ("qbittorrent", include_str!("../catalog/qbittorrent.toml")),
    ("transmission", include_str!("../catalog/transmission.toml")),
    ("deluge", include_str!("../catalog/deluge.toml")),
    ("rtorrent", include_str!("../catalog/rtorrent.toml")),
    ("torrent-blackhole", include_str!("../catalog/torrent-blackhole.toml")),
    ("sabnzbd", include_str!("../catalog/sabnzbd.toml")),
    ("nzbget", include_str!("../catalog/nzbget.toml")),
    ("usenet-blackhole", include_str!("../catalog/usenet-blackhole.toml")),
    ("cadenza", include_str!("../catalog/cadenza.toml")),
    ("translator", include_str!("../catalog/translator.toml")),
    ("jellyfin", include_str!("../catalog/jellyfin.toml")),
    ("plex", include_str!("../catalog/plex.toml")),
    ("navidrome", include_str!("../catalog/navidrome.toml")),
];

/// The catalog's manifests, read once. One that does not read is left out with an error in the log; the
/// catalog test makes sure none does.
pub fn catalog() -> &'static [Manifest] {
    static CATALOG: OnceLock<Vec<Manifest>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        FILES
            .iter()
            .filter_map(|(id, text)| match parse_for(text, Origin::Catalog) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::error!("the catalog's {id} plugin does not read: {e}");
                    None
                }
            })
            .collect()
    })
}

/// The catalog's sources that the picker and the request worker reach through the registry, as they reach a
/// plugin program's (encore_requests::sources::plugin): no hand-written path of their own. Their adapters (the
/// builtin crates') answer `search` and `download` with the SDK's shapes.
pub const THROUGH_REGISTRY: [&str; 2] = ["zotify", "gamdl"];

/// A catalog plugin by id.
pub fn find(id: &str) -> Option<&'static Manifest> {
    catalog().iter().find(|m| m.plugin.id == id)
}

/// A catalog plugin's manifest text (what the plugins table keeps in `manifest`).
pub fn text(id: &str) -> Option<&'static str> {
    FILES.iter().find(|(i, _)| *i == id).map(|(_, t)| *t)
}

/// The catalog plugin that took over a tool of the old Download tools card (`/api/system/tools/{id}`).
pub fn for_old_tool(tool: &str) -> Option<&'static Manifest> {
    catalog().iter().find(|m| m.migration.as_ref().and_then(|g| g.from_tool.as_deref()) == Some(tool))
}

/// The catalog plugin that answers for a source id of `source_order` or the picker (`deezer` → streamrip).
pub fn for_source(source: &str) -> Option<&'static Manifest> {
    catalog().iter().find(|m| m.source_ids().iter().any(|s| s.id == source || s.picker.as_deref() == Some(source)))
}
