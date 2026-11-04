//! One module per web/js/pages/*.js file.

pub mod activity;
pub mod artwork;
pub mod auth;
pub mod bugreports;
pub mod collab;
pub mod common;
pub mod contribute;
pub mod discover;
pub mod download;
pub mod downloads;
/// System → Files (0.33): the admin file manager (no web/js page; written for web-rs).
pub mod files;
pub mod foryou;
pub mod home;
pub mod leaderboard;
pub mod library;
/// Library types, Select / Delete… and who added it (0.36): the pure helpers and their words.
pub mod library_types;
pub mod own_file;
pub mod lyrics;
pub mod picker;
/// Sorting a playlist's songs (Custom order, Date added, Title, Artist, Album, Duration), kept per playlist.
pub mod playlist_sort;
pub mod playlists;
pub mod report;
pub mod requests;
/// "Get the rest of the album" for albums MusicBrainz lists and for those it doesn't.
pub mod rest_of_album;
pub mod soulseek;
pub mod storage;
pub mod translate;
pub mod youtube;
