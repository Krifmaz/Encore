//! A field's visibility rule from settings.toml: `key=value` (shown while the select `key` has that value)
//! or `key!=value` (shown while it has any other). The kit's settings crate reads and checks them.
pub use kit_server_settings::when::{parse, When};
