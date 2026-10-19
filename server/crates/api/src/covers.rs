//! The tests of album covers through the API (the index, thumbnails and colours live in crates/media,
//! `encore_media::covers`; answers gain `cover_v` and `cover_color` through the enrichment pass, crate::enrich).

#[cfg(test)]
#[path = "covers_tests.rs"]
mod covers_tests;
