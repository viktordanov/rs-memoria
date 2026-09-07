//! Strict configuration reading built on the YAML subset parser.

use memoria_application::config::{RootConfig, SidecarConfig};
use memoria_application::ports::ConfigurationReader;

use crate::yaml::{self, Yaml};

#[derive(Debug, Default, Clone, Copy)]
pub struct YamlConfigurationReader;

fn entries(value: Yaml, context: &str) -> Result<Vec<(String, Yaml)>, String> {
    match value {
        Yaml::Map(entries) => Ok(entries),
        Yaml::Null => Ok(Vec::new()),
        _ => Err(format!("{context} must be a mapping")),
    }
}

fn string_list(value: Yaml, context: &str) -> Result<Vec<String>, String> {
    match value {
        Yaml::Null => Ok(Vec::new()),
        Yaml::Seq(items) => items
            .into_iter()
            .map(|item| match item {
                Yaml::Str(s) if !s.trim().is_empty() => Ok(s),
                Yaml::Str(_) => Err(format!("{context} entries must not be empty")),
                _ => Err(format!("{context} entries must be strings")),
            })
            .collect(),
        _ => Err(format!("{context} must be a list of strings")),
    }
}

fn version(value: Yaml) -> Result<(), String> {
    match value {
        Yaml::Int(1) => Ok(()),
        Yaml::Int(other) => Err(format!(
            "unsupported version {other}; only version 1 is supported"
        )),
        _ => Err("version must be the integer 1".to_string()),
    }
}

struct Documentation {
    instructions: Vec<String>,
    instruction_files: Vec<String>,
}

fn documentation(value: Yaml) -> Result<Documentation, String> {
    let mut doc = Documentation {
        instructions: vec![],
        instruction_files: vec![],
    };
    for (key, value) in entries(value, "documentation")? {
        match key.as_str() {
            "instructions" => doc.instructions = string_list(value, "documentation.instructions")?,
            "instruction_files" => {
                doc.instruction_files = string_list(value, "documentation.instruction_files")?
            }
            other => return Err(format!("documentation has unknown field {other:?}")),
        }
    }
    Ok(doc)
}

fn fingerprints(value: Yaml) -> Result<(), String> {
    for (key, value) in entries(value, "fingerprints")? {
        match key.as_str() {
            "default" => match value {
                Yaml::Str(s) if s == "raw" => {}
                _ => return Err("fingerprints.default must be \"raw\"; no language filters exist in this release".to_string()),
            },
            "languages" => match value {
                Yaml::Null => {}
                Yaml::Map(map) if map.is_empty() => {}
                _ => return Err("fingerprints.languages must be empty; no language filter is supported in this release".to_string()),
            },
            other => return Err(format!("fingerprints has unknown field {other:?}")),
        }
    }
    Ok(())
}

fn lint(value: Yaml) -> Result<bool, String> {
    let mut hint = true;
    for (key, value) in entries(value, "lint")? {
        match key.as_str() {
            "missing_import_hint" => match value {
                Yaml::Bool(b) => hint = b,
                _ => return Err("lint.missing_import_hint must be a boolean".to_string()),
            },
            other => return Err(format!("lint has unknown field {other:?}")),
        }
    }
    Ok(hint)
}

fn parse_text(bytes: &[u8]) -> Result<Yaml, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "configuration is not valid UTF-8".to_string())?;
    yaml::parse(text).map_err(|e| e.to_string())
}

impl ConfigurationReader for YamlConfigurationReader {
    fn parse_root(&self, bytes: &[u8]) -> Result<RootConfig, String> {
        let mut config = RootConfig::default();
        let mut has_version = false;
        for (key, value) in entries(parse_text(bytes)?, "memoria.yml")? {
            match key.as_str() {
                "version" => {
                    version(value)?;
                    has_version = true;
                }
                "ignore" => config.ignore = string_list(value, "ignore")?,
                "include" => config.include = string_list(value, "include")?,
                "documentation" => {
                    let doc = documentation(value)?;
                    config.instructions = doc.instructions;
                    config.instruction_files = doc.instruction_files;
                }
                "fingerprints" => fingerprints(value)?,
                "lint" => config.missing_import_hint = lint(value)?,
                other => return Err(format!("memoria.yml has unknown field {other:?}")),
            }
        }
        if !has_version {
            return Err("memoria.yml requires `version: 1`".to_string());
        }
        Ok(config)
    }

    fn parse_sidecar(&self, bytes: &[u8]) -> Result<SidecarConfig, String> {
        let mut config = SidecarConfig::default();
        for (key, value) in entries(parse_text(bytes)?, "README.memoria.yml")? {
            match key.as_str() {
                "version" => version(value)?,
                "ignore" => config.ignore = string_list(value, "ignore")?,
                "include" => config.include = string_list(value, "include")?,
                "documentation" => {
                    let doc = documentation(value)?;
                    config.instructions = doc.instructions;
                    config.instruction_files = doc.instruction_files;
                }
                other => return Err(format!("README.memoria.yml has unknown field {other:?}")),
            }
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_defaults_and_rejections() {
        let reader = YamlConfigurationReader;
        let config = reader.parse_root(b"version: 1\n").unwrap();
        assert_eq!(config, RootConfig::default());
        assert!(
            reader
                .parse_root(b"ignore: []\n")
                .unwrap_err()
                .contains("version")
        );
        assert!(
            reader
                .parse_root(b"version: 2\n")
                .unwrap_err()
                .contains("unsupported version")
        );
        assert!(
            reader
                .parse_root(b"version: 1\nunknown: 1\n")
                .unwrap_err()
                .contains("unknown field")
        );
        assert!(
            reader
                .parse_root(b"version: 1\nfingerprints:\n  languages:\n    python: strip\n")
                .unwrap_err()
                .contains("languages")
        );
        assert!(
            reader
                .parse_root(b"version: 1\nfingerprints:\n  default: ast\n")
                .unwrap_err()
                .contains("default")
        );
        assert!(
            reader
                .parse_root(b"version: 1\nversion: 1\n")
                .unwrap_err()
                .contains("duplicate")
        );
        let config = reader.parse_root(b"version: 1\nlint:\n  missing_import_hint: false\ndocumentation:\n  instructions: [\"Keep it short.\"]\n").unwrap();
        assert!(!config.missing_import_hint);
        assert_eq!(config.instructions, vec!["Keep it short."]);
    }

    #[test]
    fn sidecar_version_optional() {
        let reader = YamlConfigurationReader;
        let config = reader
            .parse_sidecar(b"include:\n  - \"fixtures/**\"\n")
            .unwrap();
        assert_eq!(config.include, vec!["fixtures/**"]);
        assert!(
            reader
                .parse_sidecar(b"lint: {}\n")
                .unwrap_err()
                .contains("unknown field")
        );
        assert!(reader.parse_sidecar(b"fingerprints: {}\n").is_err());
    }
}
