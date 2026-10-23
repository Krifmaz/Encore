//! Plugins: the `plugin.toml` manifest ([`manifest`]), the catalog Encore ships ([`catalog`](mod@catalog)), the registry of
//! what is installed ([`registry`], the plugins table), the `Plugin` trait ([`plugin`]), the one installer
//! ([`install`] with [`fetch`], [`archive`] and [`releases`]), the sandbox and spawn builder ([`sandbox`]), the
//! plugins' logs ([`logs`]) and the process protocol ([`process`]).
//!
//! The process protocol (protocol 1 of the plugin SDK, plugins/sdk/encore_plugin.py) is described in
//! docs/plugins/CONTRACT.md, the manifest in docs/contracts/plugin-manifest.md.
pub mod archive;
pub mod catalog;
pub mod fetch;
pub mod install;
pub mod logs;
pub mod manifest;
pub mod migrate;
pub mod plugin;
pub mod process;
pub mod registry;
pub mod releases;
pub mod sandbox;

pub use catalog::{catalog, find};
pub use manifest::{check_together, parse, parse_for, Condition, Facts, Manifest, ManifestError, Origin};
pub use plugin::{Call, Code, Downloaded, Failure, FileEntry, Plugin, Status};
pub use migrate::ProgramAt;
pub use registry::{PluginSource, Refused, Registry};
pub use sandbox::{Level, Sandbox, Spawn};

#[cfg(test)]
mod archive_tests;
#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod install_tests;
#[cfg(test)]
mod manifest_tests;
#[cfg(test)]
mod migrate_tests;
#[cfg(test)]
mod process_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod sandbox_tests;
#[cfg(test)]
pub(crate) mod test_util;
