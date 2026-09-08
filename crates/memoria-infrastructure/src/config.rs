//! Strict TOML wire types mapped into application configuration values.

use std::collections::BTreeMap;

use memoria_application::config::{CONFIG_VERSION, RootConfig, SidecarConfig};
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
    instructions: Option<Vec<String>>,
    instruction_files: Option<Vec<String>>,
}

/// The exact cutover guidance for a retired configuration key.
fn retired_key(old: &str, new: &str) -> String {
    format!(
        "documentation.{old} was replaced by documentation.{new} in configuration version {CONFIG_VERSION}. \
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

fn validate_version(version: i64) -> Result<(), String> {
    if version != CONFIG_VERSION as i64 {
        return Err(format!(
            "unsupported version {version}; only version {CONFIG_VERSION} is supported. \
This release has one clean cutover: change the version, rename documentation.instructions to \
documentation.guidance and documentation.instruction_files to documentation.guidance_files, \
then follow docs/releases/0.2.0.md."
        ));
    }
    Ok(())
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
        validate_version(wire.version)?;
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
            missing_import_hint: wire.lint.missing_import_hint,
        })
    }

    fn parse_sidecar(&self, bytes: &[u8]) -> Result<SidecarConfig, String> {
        let wire: SidecarWire = parse(bytes, SIDECAR_FILE_NAME)?;
        validate_version(wire.version)?;
        validate_documentation(&wire.documentation)?;
        validate_lists(&wire.ignore, &wire.include, &wire.documentation)?;
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
        for text in ["version = 2\n", ROOT_CONFIG_TEMPLATE] {
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
version = 2 # supported version
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
        for text in ["", "# comment only\n", "version = 2\n"] {
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
version = 2
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
    fn unknown_fields_are_rejected_at_every_level() {
        for text in [
            "unknown = 1",
            "[documentation]\nunknown = []",
            "[fingerprints]\nunknown = 'raw'",
            "[lint]\nunknown = true",
        ] {
            let error = TomlConfigurationReader
                .parse_root(format!("version = 2\n{text}").as_bytes())
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
                    .parse_root(format!("version = 2\n{text}").as_bytes())
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
                    .parse_root(format!("version = 2\n{text}").as_bytes())
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
            b"version = 3",
            b"version = -1",
            b"version = '1'",
            b"version = 2.0",
            b"version = true",
            b"version = 2\nversion = 2",
            b"version: 2\n",
            b"\xff",
            b"version = 2\nignore = ['unterminated]",
            // The retired keys fail with their exact replacement names.
            b"version = 2\n[documentation]\ninstructions = []",
            b"version = 2\n[documentation]\ninstruction_files = []",
            // A mixed configuration never selects one spelling silently.
            b"version = 2\n[documentation]\nguidance = ['a']\ninstructions = ['b']",
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
                && version_error.contains("docs/releases/0.2.0.md"),
            "{version_error}"
        );
        // A retired key names its replacement, not an unknown-field message.
        let key_error = TomlConfigurationReader
            .parse_root(b"version = 2\n[documentation]\ninstructions = []")
            .unwrap_err();
        assert!(
            key_error.contains("documentation.instructions")
                && key_error.contains("documentation.guidance"),
            "{key_error}"
        );
        assert!(!key_error.contains("unknown field"), "{key_error}");
        assert!(
            TomlConfigurationReader
                .parse_root(b"version = 2\nversion = 2")
                .unwrap_err()
                .contains("duplicate")
        );
    }
}
