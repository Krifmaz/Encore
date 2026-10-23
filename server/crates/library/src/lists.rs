//! How the library lists are ordered and which columns a song row carries: the list endpoints and the
//! side index (crate::sections) use the same tables, so a section's offset lands where the list has it.

/// The columns of a song in the library lists.
pub const TRACK_COLS: &str = "id, title, artist, album, albumartist, album_id, track, disc, year, duration, format, bitrate, verify_status, verify_detail, request_id, added_at";

/// Most artists one `/artists` call returns (the web asks without a limit: this is the whole list it shows).
pub const ARTISTS_MAX: usize = 2000;

/// The ORDER BY of `key` from `table` (`(key, expression with {d}, default direction)`), in `dir` when it is
/// "asc" or "desc"; an unknown key sorts by `default`.
pub fn order(table: &[(&str, &str, &str)], key: &str, dir: &str, default: &str) -> String {
    let (_, expr, dflt) = table.iter().find(|(k, _, _)| *k == key).or_else(|| table.iter().find(|(k, _, _)| *k == default)).unwrap();
    let d = if dir == "asc" || dir == "desc" { dir } else { dflt };
    expr.replace("{d}", &d.to_uppercase())
}

/// The album list's sorts: (key, ORDER BY with `{d}` for the direction, default direction).
pub const ALBUM_SORTS: &[(&str, &str, &str)] = &[
    ("name", "lower(albumartist) {d}, year, lower(album), album_id", "asc"),
    ("artist", "lower(albumartist) {d}, year, album_id", "asc"),
    ("album", "lower(album) {d}, album_id", "asc"),
    ("year", "year {d}, lower(albumartist), album_id", "desc"),
    ("released", "year {d}, lower(albumartist), album_id", "desc"),
    // a whole library added in one scan shares one time: the album breaks the tie, so pages and the side
    // bar's offsets (crate::sections) see one order
    ("added", "added {d}, album_id", "desc"),
    ("duration", "duration {d}, album_id", "desc"),
    ("size", "size {d}, album_id", "desc"),
];
/// The song list's sorts: (key, ORDER BY with `{d}` for the direction, default direction).
pub const TRACK_SORTS: &[(&str, &str, &str)] = &[
    ("artist", "lower(artist) {d}, lower(album), disc, track, id", "asc"),
    ("title", "lower(title) {d}, id", "asc"),
    ("album", "lower(album) {d}, disc, track, id", "asc"),
    ("added", "added_at {d}, id", "desc"),
    ("released", "year {d}, lower(artist), id", "desc"),
    ("duration", "duration {d}, id", "desc"),
];
