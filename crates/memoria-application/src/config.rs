//! Configuration values produced by the configuration reader port.

/// The only supported project configuration version.
pub const CONFIG_VERSION: u64 = 2;

/// Root `memoria.toml` after strict parsing and defaulting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootConfig {
    pub ignore: Vec<String>,
    pub include: Vec<String>,
    /// Inline project documentation guidance, in authored order.
    pub guidance: Vec<String>,
    /// Guidance files, relative to this configuration file.
    pub guidance_files: Vec<String>,
    pub missing_import_hint: bool,
}

impl Default for RootConfig {
    fn default() -> RootConfig {
        RootConfig {
            ignore: vec![],
            include: vec![],
            guidance: vec![],
            guidance_files: vec![],
            missing_import_hint: true,
        }
    }
}

/// Local `README.memoria.toml` after strict parsing and defaulting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidecarConfig {
    pub ignore: Vec<String>,
    pub include: Vec<String>,
    pub guidance: Vec<String>,
    pub guidance_files: Vec<String>,
}

/// The template written by `memoria init --apply`.
pub const ROOT_CONFIG_TEMPLATE: &str = r#"# Memoria root configuration. See docs/cli.md in the Memoria project.
version = 2

# Memoria-specific exclusions. Patterns are relative to the project root.
# Tracked generated output, snapshots, and fixtures stay selected until you
# exclude them here. Local README.memoria.toml files can restore them.
ignore = []

# Root includes restore files that a root ignore pattern excluded.
include = []

[documentation]
# Project documentation guidance. Every focused review packet shows these
# entries before the owned evidence. Guidance states your documentation
# goals, your readers, and your writing standards. It is advisory context
# for the reviewer. It never selects files and never decides freshness.
guidance = []
# Guidance files, relative to this file. Missing files are errors.
guidance_files = []

[fingerprints]
# Only raw byte hashing is supported in this release.
default = "raw"
languages = {}

[lint]
# Report local README links that have no matching import as hints.
missing_import_hint = true
"#;
