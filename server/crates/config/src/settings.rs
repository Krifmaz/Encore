//! The settings table from settings.toml (generated into settings_gen.rs by build.rs, with the kit's settings
//! crate): `Keys`, `TABLE` (what the loader reads), `SCHEMA` (the Settings pages, their cards and fields) and a
//! typed accessor per key on `Config`. `Config::schema` is the answer of GET /api/admin/settings/schema
//! (docs/contracts/settings-schema.md).
use crate::Config;
pub use kit_server_settings::table::{DefaultValue, Field, Group, KeyDef, Section, Table};
use serde_json::Value;

include!(concat!(env!("OUT_DIR"), "/settings_gen.rs"));

/// The whole table: `TABLE` and `SCHEMA`.
pub static ENCORE: Table = Table { keys: TABLE, sections: SCHEMA };

/// The table's entry for `key`.
pub fn def(key: &str) -> Option<&'static KeyDef> {
    ENCORE.def(key)
}

/// The default of `key`, as the loader uses it.
pub fn default_of(key: &str) -> Option<Value> {
    ENCORE.default_of(key)
}

impl Config {
    /// GET /api/admin/settings/schema: the pages, cards and fields of Settings → Server, with each field's
    /// words, type, options, visibility rule and whether the environment locks it. Values are not in it
    /// (GET /api/admin/settings has them).
    pub fn schema(&self) -> Value {
        kit_server_settings::schema::answer(&ENCORE, |key| self.env_locked(key))
    }
}
