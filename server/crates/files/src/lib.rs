//! The admin file manager (release 0.33): browse, search by name ([`search`]), upload, make folder, rename, move, trash and restore, edit
//! tags and import, over the folders Encore may touch ([`roots`]). The API (`/api/files/*`, admin only) is in
//! encore-api's `routes::files`; docs/contracts/client-shapes-0.31-0.33.md (§2.4, Part 4) has its JSON.
//!
//! The rules every operation keeps:
//! - every path is confined to its root ([`resolve::confine`]): absolute paths, `..`, NUL and Unicode
//!   look-alikes are refused before the disk is touched ([`names`]);
//! - nothing is written through a link: each folder on the way is opened with `O_NOFOLLOW` from the root
//!   down and the operation works on those handles ([`dirfd`]), so a check can't be raced; a link itself
//!   can be renamed, moved and trashed, its target is never touched;
//! - nothing is replaced and nothing is deleted except from Trash ([`trash`]);
//! - the library's rows follow what happens to its files, in one transaction ([`rows`]);
//! - every operation writes one line to the activity log: who, what, from and to ([`audit`]).
pub mod albums;
pub mod dirfd;
pub mod import;
pub mod list;
pub mod names;
pub mod ops;
pub mod resolve;
pub mod roots;
pub mod rows;
pub mod search;
pub mod tags;
pub mod trash;
pub mod upload;

pub use albums::backfill_added_from_log;
pub use resolve::{confine, Target};

use encore_db::{Row, RowExt};
use encore_library::Ctx;
use std::sync::{Mutex, MutexGuard};

/// Who does an operation (for the activity log and Trash's "by").
#[derive(Clone, Debug)]
pub struct Who {
    /// the user's id
    pub id: i64,
    /// their username, for the log
    pub name: String,
}

impl Who {
    /// The user a request came from.
    pub fn of(user: &Row) -> Who {
        Who { id: user.i("id"), name: user.s("username") }
    }
}

/// The lock behind [`lock`].
static LOCK: Mutex<()> = Mutex::new(());

/// One file operation at a time (a move and a restore can't pick the same free name, two uploads can't
/// finish into the same one); held for the operation, not for an upload's bytes.
pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// "library:Artist/Album" (the root's id and the path in it).
pub fn place(t: &Target) -> String {
    format!("{}:{}", t.root.id, t.rel())
}

/// The activity log's line for an operation: "File manager: alex moved library:A/B to staging:B".
pub fn audit(st: &impl Ctx, who: &Who, what: &str, from: &Target, to: Option<&Target>) {
    let line = match to {
        Some(t) => format!("{what} {} to {}", place(from), place(t)),
        None => format!("{what} {}", place(from)),
    };
    audit_text(st, who, &line);
}

/// Log one file-manager action of `who` ("File manager: alice moved …").
pub fn audit_text(st: &impl Ctx, who: &Who, what: &str) {
    st.db().log("info", &format!("File manager: {} {what}", who.name));
}
