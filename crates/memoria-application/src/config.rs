//! Configuration values produced by the configuration reader port.

/// Root `memoria.toml` after strict parsing and defaulting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootConfig {
    pub ignore: Vec<String>,
    pub include: Vec<String>,
    pub instructions: Vec<String>,
    pub instruction_files: Vec<String>,
    pub missing_import_hint: bool,
}

impl Default for RootConfig {
    fn default() -> RootConfig {
        RootConfig {
            ignore: vec![],
            include: vec![],
            instructions: vec![],
            instruction_files: vec![],
            missing_import_hint: true,
        }
    }
}

/// Local `README.memoria.toml` after strict parsing and defaulting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidecarConfig {
    pub ignore: Vec<String>,
    pub include: Vec<String>,
    pub instructions: Vec<String>,
    pub instruction_files: Vec<String>,
}

/// The template written by `memoria init`.
pub const ROOT_CONFIG_TEMPLATE: &str = r#"# Memoria root configuration. See docs/cli.md in the Memoria project.
version = 1

# Memoria-specific exclusions. Patterns are relative to the project root.
# Tracked generated output, snapshots, and fixtures stay selected until you
# exclude them here. Local README.memoria.toml files can restore them.
ignore = []

# Root includes restore files that a root ignore pattern excluded.
include = []

[documentation]
# Short writing rules shown in every focused review packet.
instructions = []
# Instruction files, relative to this file. Missing files are errors.
instruction_files = []

[fingerprints]
# Only raw byte hashing is supported in this release.
default = "raw"
languages = {}

[lint]
# Report local README links that have no matching import as hints.
missing_import_hint = true
"#;

/// The minimal root README written by `memoria init` when none exists.
pub const ROOT_README_TEMPLATE: &str = "# Project\n\nThis README owns every selected file that no nearer README explains.\n\n<!-- memoria:export id=\"summary\" -->\n## Summary\n\nDescribe what this project does in a few sentences.\n<!-- /memoria:export -->\n";
