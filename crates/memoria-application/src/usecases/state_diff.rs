//! Compare logical records from two explicit snapshots without project discovery.
use super::state_inspect;
use crate::error::{AppError, Detail, DetailMap, Outcome};
use crate::ports::StateInspector;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RENDERED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDiffReport {
    pub before: Detail,
    pub after: Detail,
    pub byte_equal: bool,
    pub changes: Vec<Detail>,
}

impl StateDiffReport {
    pub fn to_detail(&self) -> Detail {
        DetailMap::default()
            .text("kind", "state_diff")
            .with("before", self.before.clone())
            .with("after", self.after.clone())
            .bool("byte_equal", self.byte_equal)
            .bool("logical_equal", self.changes.is_empty())
            .with("changes", Detail::List(self.changes.clone()))
            .build()
    }
}

// Lists with identities become maps only for comparison. The inspection
// projection and its existing JSON contract remain unchanged.
fn identities(value: &mut Detail, field: &str) {
    match value {
        Detail::Map(map) => {
            for (key, child) in map {
                identities(child, key);
            }
        }
        Detail::List(items) if matches!(field, "files" | "imports" | "invalidations") => {
            let mut map = BTreeMap::new();
            for mut item in std::mem::take(items) {
                identities(&mut item, "");
                let key = |field| match item.get(field) {
                    Some(Detail::Text(s)) => s.clone(),
                    Some(Detail::Number(n)) => n.to_string(),
                    _ => unreachable!("strict state projection has identities"),
                };
                if field == "imports" {
                    let provider = key("document");
                    let export = key("export_id");
                    let Detail::Map(exports) = map
                        .entry(provider)
                        .or_insert_with(|| Detail::Map(BTreeMap::new()))
                    else {
                        unreachable!()
                    };
                    exports.insert(export, item);
                } else {
                    map.insert(key(if field == "files" { "path" } else { "id" }), item);
                }
            }
            *value = Detail::Map(map);
        }
        Detail::List(items) => {
            for item in items {
                identities(item, "");
            }
        }
        _ => {}
    }
}

fn compare(
    path: &mut Vec<String>,
    before: Option<&Detail>,
    after: Option<&Detail>,
    changes: &mut Vec<Detail>,
) {
    if before == after {
        return;
    }
    if let (Some(Detail::Map(a)), Some(Detail::Map(b))) = (before, after) {
        let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
        for key in keys {
            path.push(key.clone());
            compare(path, a.get(key), b.get(key), changes);
            path.pop();
        }
    } else {
        changes.push(
            DetailMap::default()
                .with("path", Detail::texts(path.clone()))
                .bool("before_present", before.is_some())
                .bool("after_present", after.is_some())
                .with("before", before.cloned().unwrap_or(Detail::Null))
                .with("after", after.cloned().unwrap_or(Detail::Null))
                .build(),
        );
    }
}

pub fn run_files(
    inspector: &dyn StateInspector,
    before: &str,
    after: &str,
) -> Result<Outcome<StateDiffReport>, AppError> {
    let a = state_inspect::run_file(inspector, before)?.data;
    let b = state_inspect::run_file(inspector, after)?.data;
    let mut old = state_inspect::state_detail(&a.inspected);
    let mut new = state_inspect::state_detail(&b.inspected);
    identities(&mut old, "");
    identities(&mut new, "");
    let mut changes = Vec::new();
    compare(&mut Vec::new(), Some(&old), Some(&new), &mut changes);
    let metadata = |report: &state_inspect::InspectReport| {
        let Detail::Map(mut map) = report.to_detail() else {
            unreachable!()
        };
        map.remove("state");
        Detail::Map(map)
    };
    let report = StateDiffReport {
        before: metadata(&a),
        after: metadata(&b),
        byte_equal: a.inspected.encoded == b.inspected.encoded,
        changes,
    };
    if report.to_detail().approximate_bytes() > MAX_RENDERED_BYTES {
        return Err(AppError::io(
            "state_comparison_limit_exceeded",
            "The comparison exceeds the 256 MiB output limit.",
        ));
    }
    Ok(Outcome::new(report, vec![]))
}
