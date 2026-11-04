//! Turns settings.toml into settings_gen.rs (in OUT_DIR) with the kit's settings crate: the `Keys` names, the
//! `TABLE` of definitions the loader reads, `SCHEMA` (pages, cards and fields) and a typed accessor per key on
//! `Config`. Anything in the file that does not fit (an unknown attribute, a type that does not match the
//! default, a card or rule that names something missing) stops the build with the reason.
use kit_server_settings::file::{generate, Codegen};

/// Names an accessor cannot take: Config's own methods (the Rust keywords are refused by the kit).
const METHODS: &[&str] =
    &["load", "reload", "def", "defs", "get", "str", "bool", "int", "float", "env_locked", "set_many", "is_masked", "public_dump", "schema"];

fn main() {
    println!("cargo:rerun-if-changed=settings.toml");
    let text = std::fs::read_to_string("settings.toml").expect("settings.toml");
    let out = match generate(&text, &Codegen { accessors_on: Some("crate::Config"), methods: METHODS }) {
        Ok(code) => code,
        Err(e) => panic!("settings.toml: {e}"),
    };
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("settings_gen.rs");
    std::fs::write(path, out).unwrap();
}
