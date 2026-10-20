//! Upload (§4.3 of the client shapes). Each multipart part is a file; its filename is the name relative to
//! the upload's folder, with `/` for the files of a folder upload ("Album/CD1/01 Song.flac"): every part of
//! it is checked as a name, the folders are made as needed (none through a link), and the bytes are
//! streamed into a part file of this upload's own in the file's folder (`.<name>.<random>.encore-upload`: a
//! new file, never through a link and never one that was there; hidden, so the scan and the import pass it
//! by), which is renamed to its name once it is complete. Only the upload that made a part file renames or
//! removes it (it checks the file is still the one it made), and a part file whose upload stopped (the
//! request dropped) is removed with it. When the name is taken: `rename` (the default) gives the new file
//! "name (2).flac", `skip` drops it, `replace` moves the file that is there to Trash first (as Move to trash
//! does) and never deletes; a folder in the way is never replaced.
use crate::dirfd::{Dir, Kind};
use crate::names::{self, FOLDER_TAKEN, TAKEN, THROUGH_LINK};
use crate::resolve::{io_error, Target};
use crate::roots::LibraryDirs;
use crate::rows::Change;
use crate::Who;
use encore_core::error::{err, ApiError};
use encore_library::Ctx;
use serde_json::{json, Value};
use std::fs::File;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    Rename,
    Replace,
    Skip,
}

impl Conflict {
    pub fn parse(s: Option<&str>) -> Result<Conflict, ApiError> {
        match s.unwrap_or("rename") {
            "" | "rename" => Ok(Conflict::Rename),
            "replace" => Ok(Conflict::Replace),
            "skip" => Ok(Conflict::Skip),
            _ => Err(err(400, "conflict is rename, replace or skip")),
        }
    }
}

/// A file being received: its part file is open in the folder it goes to. Dropped before [`finish`] took
/// it (the bytes stopped, the request went away), its part file is removed.
pub struct Incoming {
    /// the part's filename as sent (relative to the upload's folder)
    pub sent: String,
    /// the folder it goes to, from the root
    at: Target,
    dir: Dir,
    name: String,
    part: String,
    pub file: File,
    /// the device and inode of the part file this upload made
    made: (u64, u64),
    /// renamed into place (or already removed): nothing left to clean up
    done: bool,
}

impl Incoming {
    /// Is the part file still the one this upload made (no other upload's, nothing put in its place)?
    fn ours(&self) -> bool {
        self.dir.lstat(&self.part).ok().flatten().is_some_and(|s| s.kind == Kind::File && (s.dev, s.ino) == self.made)
    }

    /// Removes this upload's part file (only when it is still the one it made).
    fn remove_part(&mut self) {
        if !self.done && self.ours() {
            let _ = self.dir.unlink(&self.part);
        }
        self.done = true;
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        self.remove_part();
    }
}

/// What [`begin`] decided for one part.
pub enum Begin {
    Write(Box<Incoming>),
    /// not written (its bytes are read and dropped): the name as sent and why
    Skip(String, String),
}

/// What happened to one file, for the answer.
pub enum Outcome {
    /// written: the name as sent, its path from the root, its size, and the name it got when that differs
    Uploaded { sent: String, path: String, size: u64, renamed: Option<String> },
    Skipped { sent: String, reason: String },
}

/// A part file name of this upload's own next to `name`: hidden, with a random piece (a short one when the
/// name is too long for it).
fn part_name(name: &str) -> String {
    let rnd = encore_core::ids::random_hex(8);
    let long = format!(".{name}.{rnd}.encore-upload");
    if long.len() <= names::NAME_MAX {
        long
    } else {
        format!(".encore-upload-{rnd}")
    }
}

/// Gets ready to receive the part `sent` into the folder `base` (already checked): its names checked, its
/// folders made, its `.part` file open. A part that can't be taken is skipped with a reason; an error is
/// for a problem with the whole upload.
pub fn begin(base: &Target, sent: &str, conflict: Conflict) -> Result<Begin, ApiError> {
    let skip = |why: &str| Ok(Begin::Skip(sent.to_string(), why.to_string()));
    let parts: Vec<&str> = sent.split('/').filter(|p| !p.is_empty()).collect();
    let Some((name, folders)) = parts.split_last() else { return skip("A name can't be empty") };
    for p in &parts {
        if let Err(e) = names::check_name(p) {
            return skip(&e.detail);
        }
    }
    if parts.len() + base.parts.len() > names::DEPTH_MAX {
        return skip("That path is too deep");
    }
    let _g = crate::lock();
    let mut dir = base.open_dir()?;
    let mut at = base.clone();
    for f in folders {
        match dir.lstat(f).map_err(io_error)?.map(|s| s.kind) {
            None => dir.mkdir(f).map_err(io_error)?,
            Some(Kind::Dir) => {}
            Some(Kind::Link) => return skip(THROUGH_LINK),
            Some(_) => return skip(TAKEN),
        }
        dir = dir.child(f).map_err(io_error)?;
        at = at.child(f);
    }
    match (dir.lstat(name).map_err(io_error)?.map(|s| s.kind), conflict) {
        (Some(Kind::Dir), Conflict::Skip | Conflict::Replace) => return skip(FOLDER_TAKEN),
        (Some(_), Conflict::Skip) => return skip(TAKEN),
        _ => {}
    }
    // a new part file (O_EXCL): a name that is taken is never removed, another random one is tried
    let mut tries = 0;
    let (part, file) = loop {
        let part = part_name(name);
        match dir.create_new(&part) {
            Ok(f) => break (part, f),
            Err(e) if e.errno == Some(libc::EEXIST) && tries < 8 => tries += 1,
            Err(e) => return Err(io_error(e)),
        }
    };
    let made = match file.metadata() {
        Ok(m) => (std::os::unix::fs::MetadataExt::dev(&m), std::os::unix::fs::MetadataExt::ino(&m)),
        Err(e) => {
            let _ = dir.unlink(&part);
            return Err(err(500, format!("Couldn't write {sent}: {e}")));
        }
    };
    Ok(Begin::Write(Box::new(Incoming { sent: sent.to_string(), at, dir, name: name.to_string(), part, file, made, done: false })))
}

/// The part's bytes didn't all arrive (or couldn't be written): its part file goes (only this upload's own).
pub fn abort(mut inc: Incoming) {
    inc.remove_part();
}

/// The part is complete (`size` bytes, flushed): it gets its name as `conflict` says, and the library reads
/// it in when it landed in the library (through `change`). Only the part file this upload made is renamed:
/// one that isn't it any more is left alone and the file is reported as not uploaded.
pub fn finish(st: &impl Ctx, who: &Who, mut inc: Incoming, size: u64, conflict: Conflict, change: &mut Change) -> Outcome {
    let _g = crate::lock();
    let skipped = |inc: &mut Incoming, reason: &str| {
        inc.remove_part();
        Outcome::Skipped { sent: inc.sent.clone(), reason: reason.to_string() }
    };
    if !inc.ours() {
        inc.done = true;
        return Outcome::Skipped { sent: inc.sent.clone(), reason: "Its part file was changed by something else while it was uploading: send it again".into() };
    }
    let (dir, name) = (&inc.dir, inc.name.clone());
    let there = match dir.lstat(&name) {
        Ok(s) => s.map(|s| s.kind),
        Err(e) => return skipped(&mut inc, &io_error(e).detail),
    };
    let mut final_name = name.clone();
    match (there, conflict) {
        (None, _) => {}
        (Some(Kind::Dir), Conflict::Skip | Conflict::Replace) => return skipped(&mut inc, FOLDER_TAKEN),
        (Some(_), Conflict::Skip) => return skipped(&mut inc, TAKEN),
        (Some(_), Conflict::Rename) => {
            final_name = names::free_name(&name, false, |n| dir.lstat(n).ok().flatten().is_some());
        }
        (Some(_), Conflict::Replace) => {
            // the file that is there goes to Trash first; when it can't, the new one is dropped and the old stays
            if crate::trash::trash_one(st, who, &inc.at.child(&name), dir, &name, change).is_err() {
                return skipped(&mut inc, "Couldn't move the old file to Trash");
            }
        }
    }
    // the part file is still open (held by `inc`) while it is renamed
    if let Err(e) = inc.dir.rename_noreplace(&inc.part, &inc.dir, &final_name) {
        return skipped(&mut inc, &io_error(e).detail);
    }
    inc.done = true;
    let _ = inc.dir.sync();
    let dirs = LibraryDirs::of(st.cfg());
    change.arrived(&dirs, &inc.dir.path.join(&final_name));
    let placed = inc.at.child(&final_name);
    crate::audit(st, who, "uploaded", &placed, None);
    let sent = inc.sent.clone();
    let renamed = (final_name != name).then(|| {
        let mut s: Vec<&str> = sent.split('/').filter(|p| !p.is_empty()).collect();
        s.pop();
        s.push(&final_name);
        s.join("/")
    });
    Outcome::Uploaded { sent, path: placed.rel(), size, renamed }
}

/// The answer for all parts: `{uploaded: [{name, path, size}], renamed: [{from, to}], skipped: [{name, reason}]}`.
pub fn answer(outcomes: &[Outcome]) -> Value {
    let (mut up, mut ren, mut skip) = (vec![], vec![], vec![]);
    for o in outcomes {
        match o {
            Outcome::Uploaded { sent, path, size, renamed } => {
                up.push(json!({ "name": sent, "path": path, "size": size }));
                if let Some(to) = renamed {
                    ren.push(json!({ "from": sent, "to": to }));
                }
            }
            Outcome::Skipped { sent, reason } => skip.push(json!({ "name": sent, "reason": reason })),
        }
    }
    json!({ "uploaded": up, "renamed": ren, "skipped": skip })
}
