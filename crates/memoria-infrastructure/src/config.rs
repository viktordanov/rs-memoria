//! Strict TOML wire types mapped into application configuration values.

use std::collections::BTreeMap;

use memoria_application::config::{CONFIG_VERSION, RootConfig, SidecarConfig, SidecarRejection};
use memoria_application::ports::ConfigurationReader;
use memoria_application::snapshot::{ROOT_CONFIG_PATH, SIDECAR_FILE_NAME};
use serde::Deserialize;

#[derive(Debug, Default, Clone, Copy)]
pub struct TomlConfigurationReader;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RootWire {
    version: i64,
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default, deserialize_with = "table")]
    documentation: DocumentationWire,
    #[serde(default, deserialize_with = "table")]
    fingerprints: FingerprintsWire,
    #[serde(default, deserialize_with = "table")]
    lint: LintWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SidecarWire {
    #[serde(default = "inherited_version")]
    version: i64,
    #[serde(default)]
    ignore: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default, deserialize_with = "table")]
    documentation: DocumentationWire,
}

fn inherited_version() -> i64 {
    CONFIG_VERSION as i64
}

/// The documentation section. The version 1 keys are still parsed, so a
/// project that has not finished the cutover receives the exact replacement
/// name instead of an unknown-field message.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DocumentationWire {
    guidance: Vec<String>,
    guidance_files: Vec<String>,
    /// Root only. A sidecar that sets it is refused.
    section_guidance_files: Option<Vec<String>>,
    instructions: Option<Vec<String>>,
    instruction_files: Option<Vec<String>>,
}

/// The exact cutover guidance for a retired configuration key.
fn retired_key(old: &str, new: &str) -> String {
    format!(
        "documentation.{old} was replaced by documentation.{new} in configuration version 2. \
Rename the key. See the clean cutover in docs/releases/0.2.0.md."
    )
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FingerprintsWire {
    default: String,
    languages: BTreeMap<String, String>,
}

impl Default for FingerprintsWire {
    fn default() -> Self {
        Self {
            default: "raw".to_string(),
            languages: BTreeMap::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LintWire {
    missing_import_hint: bool,
}

impl Default for LintWire {
    fn default() -> Self {
        Self {
            missing_import_hint: true,
        }
    }
}

fn validate_version(version: i64, file: &str) -> Result<(), String> {
    match version {
        v if v == CONFIG_VERSION as i64 => Ok(()),
        2 => Err(format!(
            "{file} declares version 2. Memoria 0.7 reads configuration version {CONFIG_VERSION}: \
each document covers its own folder and below, and a document hands a subfolder to a tracked \
document there only through a link or an import. Change `version = 2` to `version = {CONFIG_VERSION}` \
in memoria.toml and in any README.memoria.toml that declares a version. Then run `memoria status` \
and read the review queue before you acknowledge anything. See \u{201c}Migration to 0.7.0\u{201d} in \
the Memoria changelog."
        )),
        1 => Err(format!(
            "unsupported version 1; this release reads version {CONFIG_VERSION} only. \
Configuration version 1 is retired: rename documentation.instructions to \
documentation.guidance and documentation.instruction_files to documentation.guidance_files, \
follow docs/releases/0.2.0.md, then set `version = {CONFIG_VERSION}` and read \
\u{201c}Migration to 0.7.0\u{201d} in the Memoria changelog."
        )),
        other => Err(format!(
            "unsupported version {other}; this release reads version {CONFIG_VERSION} only."
        )),
    }
}

/// Reject the retired keys before any value is used, so a mixed
/// configuration never silently selects one of the two spellings.
fn validate_documentation(documentation: &DocumentationWire) -> Result<(), String> {
    if documentation.instructions.is_some() {
        return Err(retired_key("instructions", "guidance"));
    }
    if documentation.instruction_files.is_some() {
        return Err(retired_key("instruction_files", "guidance_files"));
    }
    Ok(())
}

fn validate_lists(
    ignore: &[String],
    include: &[String],
    documentation: &DocumentationWire,
) -> Result<(), String> {
    for (name, values) in [
        ("ignore", ignore),
        ("include", include),
        ("documentation.guidance", &documentation.guidance),
        (
            "documentation.guidance_files",
            &documentation.guidance_files,
        ),
        (
            "documentation.section_guidance_files",
            documentation
                .section_guidance_files
                .as_deref()
                .unwrap_or_default(),
        ),
    ] {
        if values.iter().any(|value| value.trim().is_empty()) {
            return Err(format!("{name} entries must not be empty"));
        }
    }
    Ok(())
}

// Serde structs can deserialize from sequences. Configuration sections must
// instead be TOML tables, including when all their fields have defaults.
fn table<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    toml::Table::deserialize(deserializer)?
        .try_into()
        .map_err(serde::de::Error::custom)
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], source: &str) -> Result<T, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "configuration is not valid UTF-8".to_string())?;
    toml::from_str(text).map_err(|error| format!("{source}: {error}"))
}

impl ConfigurationReader for TomlConfigurationReader {
    fn parse_root(&self, bytes: &[u8]) -> Result<RootConfig, String> {
        let wire: RootWire = parse(bytes, ROOT_CONFIG_PATH)?;
        validate_version(wire.version, ROOT_CONFIG_PATH)?;
        validate_documentation(&wire.documentation)?;
        validate_lists(&wire.ignore, &wire.include, &wire.documentation)?;
        if wire.fingerprints.default != "raw" {
            return Err(
                "fingerprints.default must be \"raw\"; no language filters exist in this release"
                    .to_string(),
            );
        }
        if !wire.fingerprints.languages.is_empty() {
            return Err("fingerprints.languages must be empty; no language filter is supported in this release".to_string());
        }
        Ok(RootConfig {
            ignore: wire.ignore,
            include: wire.include,
            guidance: wire.documentation.guidance,
            guidance_files: wire.documentation.guidance_files,
            section_guidance_files: wire
                .documentation
                .section_guidance_files
                .unwrap_or_default(),
            missing_import_hint: wire.lint.missing_import_hint,
        })
    }

    fn parse_sidecar(&self, bytes: &[u8]) -> Result<SidecarConfig, SidecarRejection> {
        let invalid = SidecarRejection::Invalid;
        let wire: SidecarWire = parse(bytes, SIDECAR_FILE_NAME).map_err(invalid)?;
        validate_version(wire.version, SIDECAR_FILE_NAME).map_err(invalid)?;
        validate_documentation(&wire.documentation).map_err(invalid)?;
        if wire.documentation.section_guidance_files.is_some() {
            return Err(SidecarRejection::RootOnlyKey(format!(
                "documentation.section_guidance_files is allowed only in {ROOT_CONFIG_PATH}. Move the registration to the root {ROOT_CONFIG_PATH}; any document can name a guide registered there."
            )));
        }
        validate_lists(&wire.ignore, &wire.include, &wire.documentation).map_err(invalid)?;
        Ok(SidecarConfig {
            ignore: wire.ignore,
            include: wire.include,
            guidance: wire.documentation.guidance,
            guidance_files: wire.documentation.guidance_files,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memoria_application::config::ROOT_CONFIG_TEMPLATE;

    #[test]
    fn root_defaults_and_init_template() {
        for text in ["version = 3\n", ROOT_CONFIG_TEMPLATE] {
            assert_eq!(
                TomlConfigurationReader.parse_root(text.as_bytes()).unwrap(),
                RootConfig::default()
            );
        }
        assert_eq!(ROOT_CONFIG_PATH, "memoria.toml");
        assert_eq!(SIDECAR_FILE_NAME, "README.memoria.toml");
    }

    #[test]
    fn root_reads_all_fields_and_preserves_exact_strings() {
        let config = TomlConfigurationReader
            .parse_root(
                br#"
version = 3 # supported version
ignore = ["**/generated/**", 'build/**']
include = ['fixtures/**']
[documentation]
guidance = ["Style: use \"plain\" words # literally.", """
Explain the boundary.
Keep the trailing newline.
""", '''Use 'single' quotes.''']
guidance_files = ['.agents/writing.md', 'rules with spaces.md']
[fingerprints]
default = 'raw'
languages = {}
[lint]
missing_import_hint = false
"#,
            )
            .unwrap();
        assert_eq!(config.ignore, ["**/generated/**", "build/**"]);
        assert_eq!(config.include, ["fixtures/**"]);
        assert_eq!(
            config.guidance,
            [
                "Style: use \"plain\" words # literally.",
                "Explain the boundary.\nKeep the trailing newline.\n",
                "Use 'single' quotes.",
            ]
        );
        assert_eq!(
            config.guidance_files,
            [".agents/writing.md", "rules with spaces.md"]
        );
        assert!(!config.missing_import_hint);
    }

    #[test]
    fn sidecar_defaults_optional_version_and_fields() {
        for text in ["", "# comment only\n", "version = 3\n"] {
            assert_eq!(
                TomlConfigurationReader
                    .parse_sidecar(text.as_bytes())
                    .unwrap(),
                SidecarConfig::default()
            );
        }
        let config = TomlConfigurationReader
            .parse_sidecar(
                br#"
version = 3
ignore = ['*.tmp']
include = ['fixtures/**']
[documentation]
guidance = ['''Local rule:
Use "plain" words. # literal'''] # comment
guidance_files = ['local rules.md']
"#,
            )
            .unwrap();
        assert_eq!(config.ignore, ["*.tmp"]);
        assert_eq!(config.include, ["fixtures/**"]);
        assert_eq!(
            config.guidance,
            ["Local rule:\nUse \"plain\" words. # literal"]
        );
        assert_eq!(config.guidance_files, ["local rules.md"]);
    }

    #[test]
    fn section_guides_are_registered_in_the_root_only() {
        let config = TomlConfigurationReader
            .parse_root(
                b"version = 3\n[documentation]\nsection_guidance_files = ['docs/templates/a.md', 'b.md']\n",
            )
            .unwrap();
        assert_eq!(
            config.section_guidance_files,
            ["docs/templates/a.md", "b.md"]
        );
        assert!(
            TomlConfigurationReader
                .parse_root(b"version = 3\n[documentation]\nsection_guidance_files = ['']\n")
                .is_err()
        );
        // A sidecar registration is a configuration error that names the root.
        for text in [
            "[documentation]\nsection_guidance_files = []\n",
            "[documentation]\nsection_guidance_files = ['a.md']\n",
        ] {
            let error = TomlConfigurationReader
                .parse_sidecar(text.as_bytes())
                .unwrap_err();
            assert_eq!(error.code(), "configuration_invalid", "{text}");
            assert!(
                error
                    .message()
                    .contains("section_guidance_files is allowed only in memoria.toml"),
                "{}",
                error.message()
            );
        }
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        for text in [
            "unknown = 1",
            "[documentation]\nunknown = []",
            "[fingerprints]\nunknown = 'raw'",
            "[lint]\nunknown = true",
        ] {
            let error = TomlConfigurationReader
                .parse_root(format!("version = 3\n{text}").as_bytes())
                .unwrap_err();
            assert!(error.contains("unknown field"), "{error}");
        }
        for text in [
            "unknown = 1",
            "[documentation]\nunknown = []",
            "[lint]",
            "[fingerprints]",
        ] {
            let error = TomlConfigurationReader
                .parse_sidecar(text.as_bytes())
                .unwrap_err();
            assert_eq!(error.code(), "sidecar_invalid");
            let error = error.message();
            assert!(error.contains("unknown field"), "{error}");
        }
    }

    #[test]
    fn invalid_types_and_empty_entries_are_rejected() {
        for text in [
            "ignore = 'x'",
            "include = [1]",
            "ignore = [' ']",
            "include = ['']",
            "documentation = []",
            "documentation.guidance = [false]",
            "documentation.guidance = [' ']",
            "documentation.guidance_files = 'a.md'",
            "documentation.guidance_files = ['']",
        ] {
            assert!(
                TomlConfigurationReader
                    .parse_root(format!("version = 3\n{text}").as_bytes())
                    .is_err(),
                "{text}"
            );
            assert!(
                TomlConfigurationReader
                    .parse_sidecar(text.as_bytes())
                    .is_err(),
                "{text}"
            );
        }
        for text in [
            "fingerprints = []",
            "fingerprints.default = 1",
            "fingerprints.default = 'ast'",
            "fingerprints.languages = []",
            "fingerprints.languages.python = 'strip'",
            "lint = false",
            "lint.missing_import_hint = 'false'",
        ] {
            assert!(
                TomlConfigurationReader
                    .parse_root(format!("version = 3\n{text}").as_bytes())
                    .is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn version_syntax_duplicates_and_encoding_are_strict() {
        assert!(
            TomlConfigurationReader
                .parse_root(b"")
                .unwrap_err()
                .contains("version")
        );
        for bytes in [
            &b"version = 1"[..],
            b"version = 2",
            b"version = 4",
            b"version = -1",
            b"version = '1'",
            b"version = 3.0",
            b"version = true",
            b"version = 3\nversion = 3",
            b"version: 2\n",
            b"\xff",
            b"version = 3\nignore = ['unterminated]",
            // The retired keys fail with their exact replacement names.
            b"version = 3\n[documentation]\ninstructions = []",
            b"version = 3\n[documentation]\ninstruction_files = []",
            // A mixed configuration never selects one spelling silently.
            b"version = 3\n[documentation]\nguidance = ['a']\ninstructions = ['b']",
        ] {
            assert!(
                TomlConfigurationReader.parse_root(bytes).is_err(),
                "{bytes:?}"
            );
            assert!(
                TomlConfigurationReader.parse_sidecar(bytes).is_err(),
                "{bytes:?}"
            );
        }
        // The version error names the exact cutover procedure.
        let version_error = TomlConfigurationReader
            .parse_root(b"version = 1")
            .unwrap_err();
        assert!(
            version_error.contains("unsupported version 1"),
            "{version_error}"
        );
        assert!(
            version_error.contains("documentation.guidance")
                && version_error.contains("docs/releases/0.2.0.md")
                && version_error.contains("version = 3"),
            "{version_error}"
        );
        // Version 2 names the 0.7 cutover exactly, for the root and for a
        // sidecar that declares a version.
        for error in [
            TomlConfigurationReader
                .parse_root(b"version = 2")
                .unwrap_err(),
            TomlConfigurationReader
                .parse_sidecar(b"version = 2")
                .unwrap_err()
                .message()
                .to_string(),
        ] {
            assert!(
                error.contains("declares version 2. Memoria 0.7 reads configuration version 3:")
                    && error.contains("Change `version = 2` to `version = 3`")
                    && error.contains("README.memoria.toml")
                    && error.contains("Migration to 0.7.0"),
                "{error}"
            );
        }
        assert_eq!(
            TomlConfigurationReader
                .parse_root(b"version = 4")
                .unwrap_err(),
            "unsupported version 4; this release reads version 3 only."
        );
        // A retired key names its replacement, not an unknown-field message.
        let key_error = TomlConfigurationReader
            .parse_root(b"version = 3\n[documentation]\ninstructions = []")
            .unwrap_err();
        assert!(
            key_error.contains("documentation.instructions")
                && key_error.contains("documentation.guidance"),
            "{key_error}"
        );
        assert!(!key_error.contains("unknown field"), "{key_error}");
        assert!(
            TomlConfigurationReader
                .parse_root(b"version = 3\nversion = 3")
                .unwrap_err()
                .contains("duplicate")
        );
    }
}
