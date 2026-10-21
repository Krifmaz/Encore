//! Download sources: the builtin plugins' code (Soulseek, torrents and Usenet with the download clients,
//! streamrip, yt-dlp, Amazon links) and the sources of plugin programs ([`plugin`]).
pub mod amazon;
pub mod dl;
pub mod links;
pub mod plugin;
pub mod slskd;
pub mod streamrip;
pub mod torznab;
pub mod ytdlp;
