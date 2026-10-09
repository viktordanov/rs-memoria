//! Configuration values produced by the configuration reader port.

/// The only supported project configuration version.
pub const CONFIG_VERSION: u64 = 3;

/// Root `memoria.toml` after strict parsing and defaulting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootConfig {
    pub ignore: Vec<String>,
    pub include: Vec<String>,
    /// Inline project documentation guidance, in authored order.
    pub guidance: Vec<String>,
    /// Guidance files, relative to this configuration file.
    pub guidance_files: Vec<String>,
    /// Registered section guides, relative to this configuration file. A
    /// registration reserves the file. It applies to a document only through
    /// a section marker that names it.
    pub section_guidance_files: Vec<String>,
    pub missing_import_hint: bool,
}

impl Default for RootConfig {
    fn default() -> RootConfig {
        RootConfig {
            ignore: vec![],
            include: vec![],
            guidance: vec![],
            guidance_files: vec![],
            section_guidance_files: vec![],
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

/// Why a `README.memoria.toml` sidecar was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarRejection {
    /// The sidecar does not parse or validate (`sidecar_invalid`).
    Invalid(String),
    /// The sidecar sets a key that only the root `memoria.toml` may set
    /// (`configuration_invalid`).
    RootOnlyKey(String),
}

impl SidecarRejection {
    /// The diagnostic code the snapshot reports.
    pub fn code(&self) -> &'static str {
        match self {
            SidecarRejection::Invalid(_) => "sidecar_invalid",
            SidecarRejection::RootOnlyKey(_) => "configuration_invalid",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            SidecarRejection::Invalid(message) | SidecarRejection::RootOnlyKey(message) => message,
        }
    }
}

/// The template written by `memoria init --apply`.
pub const ROOT_CONFIG_TEMPLATE: &str = r#"# Memoria root configuration. See docs/cli.md in the Memoria project.
# Each document covers its folder and below. Link or import a document in a
# subfolder to hand that subfolder to it.
version = 3

# Memoria-specific exclusions. Patterns are relative to the project root.
# Tracked generated output, snapshots, and fixtures stay selected until you
# exclude them here. Local README.memoria.toml files can restore them.
ignore = []

# Root includes restore files that a root ignore pattern excluded.
include = []

[documentation]
# Project documentation guidance. Every review names these sources, and
# `memoria guidance <DOCUMENT>` prints the text. Guidance states your
# documentation goals, your readers, and your writing standards. It is
# advisory context for the reviewer. It never selects files and never
# decides freshness.
guidance = []
# Guidance files, relative to this file. Missing files are errors.
guidance_files = []

[fingerprints]
# Only raw byte hashing is supported in this release.
default = "raw"
languages = {}

[lint]
# Report local links to documents that have no matching import as hints.
missing_import_hint = true
"#;
