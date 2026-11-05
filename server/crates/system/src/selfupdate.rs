//! "Update now", the container part: the kit's self-update (`kit_server_selfupdate`) with Encore's facts.
//! `encore self-update-helper --target <id> --image <image> --schema <n>` runs in a short-lived container of
//! the NEW image that the server starts through the Docker socket (update.rs) and swaps the server's
//! container: stop, rename, create under the old name with the same settings, start, and roll back when the
//! new one does not keep running – or, when the new server had begun migrating the database, start it again
//! instead, since the old Encore may not read the migrated database. Every step is a line
//! `encore self-update: <step>: <text>` that update_report.rs reads back.
use kit_server_selfupdate::App;

/// The only image Encore updates itself to.
pub const IMAGE: &str = "ghcr.io/krifmaz/encore";
/// The program inside the image (Dockerfile: COPY … /usr/local/bin/encore).
const BINARY: &str = "/usr/local/bin/encore";
/// The oldest image that has the self-update helper in it.
const FIRST_SELF_UPDATING: &str = "0.28.0";
/// Where the backups taken before a migration are kept.
const BACKUPS: &str = "/config/backups";

/// Encore as the kit's self-update takes it: its image and program (so the helper's label is
/// `org.encore.role` and its lines start with `encore self-update: `, as before the kit), this build's
/// version and database schema, what its exit code for a newer database means, and how its log shows a
/// migration.
pub fn app() -> App {
    App::new("Encore", IMAGE, BINARY)
        .version(option_env!("ENCORE_VERSION"))
        .schema(Some(encore_db::SCHEMA_VERSION))
        .first_self_updating(FIRST_SELF_UPDATING)
        .backups(BACKUPS)
        .exit_meaning(encore_db::EXIT_NEWER_DB, "the new Encore refused the database because a newer Encore wrote it (the image is older code than the one that was running)")
        .migration_in_log(encore_db::migration_in_log)
}

/// `encore self-update-helper …`: Some(exit code) when the program was started as the helper, None otherwise
/// (then it is the server).
pub async fn main_if_helper() -> Option<i32> {
    kit_server_selfupdate::swap::main_if_helper(&app()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helpers and records of Encore before the kit are still found and read.
    #[test]
    fn encore_keeps_its_label_and_line_prefix() {
        let a = app();
        assert_eq!((a.label.as_str(), a.line_prefix.as_str()), ("org.encore.role", "encore self-update: "));
        assert_eq!(a.schema, Some(encore_db::SCHEMA_VERSION));
        assert!(kit_server_selfupdate::swap::exit_meaning(&a, i64::from(encore_db::EXIT_NEWER_DB)).unwrap().contains("refused the database"));
        assert_eq!((a.migration_in_log.unwrap())(&encore_db::migrating_line(42, 43)), Some((42, 43)));
    }
}
