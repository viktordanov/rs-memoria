//! Validated, offline reading projections. None is an acknowledgement packet.
use crate::error::{AppError, Detail, DetailMap};
use crate::packet::FocusedReviewPacket;
use crate::ports::{
    FingerprintHasher, PacketFailure, PacketInput, PacketSource, ReviewPacketCodec,
};

pub fn run(
    input: &dyn PacketInput,
    codec: &dyn ReviewPacketCodec,
    hasher: &dyn FingerprintHasher,
    source: &PacketSource,
    section: &str,
    file: Option<&str>,
    trust: bool,
) -> Result<Detail, AppError> {
    if ![
        "summary",
        "changes",
        "guidance",
        "content",
        "history",
        "inventory",
        "incremental",
    ]
    .contains(&section)
    {
        return Err(AppError::usage(
            "packet_view_section_invalid",
            "Unknown packet view section.",
        ));
    }
    if trust && (section != "incremental" || file.is_some()) {
        return Err(AppError::usage(
            "packet_view_trust_invalid",
            "--trust-prior-review requires --section incremental.",
        ));
    }
    let failure = |e| match e {
        PacketFailure::Io(e) => AppError::io("packet_unreadable", e.to_string()),
        PacketFailure::Invalid { code, message } => AppError::usage(code, message),
    };
    let bytes = input.read(source).map_err(failure)?;
    let packet = codec.decode(&bytes).map_err(failure)?;
    super::ack::verify_packet_with_hasher(hasher, &packet)?;
    project(&packet, section, file, trust)
}

fn get(value: &Detail, key: &str) -> Detail {
    value.get(key).cloned().unwrap_or(Detail::Null)
}

fn project(
    p: &FocusedReviewPacket,
    section: &str,
    file: Option<&str>,
    trust: bool,
) -> Result<Detail, AppError> {
    let data = p.to_detail();
    let content = get(&data, "content");
    let context = get(&data, "context");
    let selected = if let Some(path) = file {
        let readme = get(&content, "readme");
        let files = get(&content, "files");
        let Detail::List(files) = files else {
            unreachable!()
        };
        std::iter::once(readme)
            .chain(files)
            .find(|f| f.get("path") == Some(&Detail::text(path)))
            .ok_or_else(|| {
                AppError::usage(
                    "packet_view_file_missing",
                    "The path is absent from this saved packet.",
                )
            })?
    } else {
        match section {
            "guidance" => get(&context, "guidance"),
            "content" => content,
            "history" => DetailMap::default()
                .with("previous_review", get(&context, "previous_review"))
                .with("diffs", get(&context, "diffs"))
                .with("git", get(&context, "git"))
                .build(),
            "inventory" => get(&data, "manifest"),
            "incremental" => incremental(p, &data, trust),
            _ => {
                let Detail::List(diffs) = get(&context, "diffs") else {
                    unreachable!()
                };
                DetailMap::default()
                    .with("changes", get(&context, "changes"))
                    .with("diffs", Detail::list(diffs.into_iter().map(|mut d| {
                        if let Detail::Map(m) = &mut d { m.remove("old_body"); m.remove("old_encoding"); }
                        d
                    })))
                    .number("owned_files", p.content.files.len() as u64)
                    .number("raw_input_bytes", p.raw_input_bytes)
                    .with("invalidations", get(&data, "covered_invalidations"))
                    .text("guidance_digest", p.context.guidance.digest.to_hex())
                    .text("review_obligation", "Full review unless the owner explicitly authorizes experimental P1. Retrieve required guidance and content.")
                    .build()
            }
        }
    };
    Ok(DetailMap::default()
        .text("kind", "packet_view")
        .number("view_version", 1)
        .bool("canonical", false)
        .text("document", p.document.as_str())
        .text("snapshot_token", &p.token)
        .text("source_packet_digest", &p.packet_digest)
        .text("section", if file.is_some() { "file" } else { section })
        .with("selection", selected)
        .build())
}

fn incremental(p: &FocusedReviewPacket, data: &Detail, trust: bool) -> Detail {
    let mut reasons = Vec::new();
    if !trust {
        reasons.push("prior_review_not_trusted");
    }
    match &p.context.previous_review {
        None => reasons.push("new_boundary"),
        Some(previous) => {
            if previous.guidance != p.context.guidance.digest {
                reasons.push("guidance_changed");
            }
            if previous.manifest.policy_hash != p.manifest.policy_hash {
                reasons.push("selection_policy_changed");
            }
        }
    }
    if !p.covered_invalidations.is_empty() {
        reasons.push("semantic_invalidation");
    }
    if p.context
        .changes
        .iter()
        .any(|c| c.kind == "import" || (c.kind == "file" && c.change != "changed"))
    {
        reasons.push("ownership_or_import_set_changed");
    }
    if !p.context.changes.is_empty() && !p.context.consumers.is_empty() {
        reasons.push("dependent_effects_require_full_review");
    }
    if p.context.diffs.iter().any(|d| d.status != "available") {
        reasons.push("historical_evidence_unavailable");
    }
    let eligible = reasons.is_empty();
    let content = get(data, "content");
    let context = get(data, "context");
    let changed = |path: &str| {
        p.context
            .changes
            .iter()
            .any(|c| c.kind == "file" && c.identity == path)
    };
    let Detail::List(files) = get(&content, "files") else {
        unreachable!()
    };
    let files = if eligible {
        files
            .into_iter()
            .zip(&p.content.files)
            .filter_map(|(d, f)| changed(&f.path).then_some(d))
            .collect()
    } else {
        files
    };
    DetailMap::default().text("policy", "P1-experimental").bool("model_quality_gate_passed", false)
        .bool("full_review_required", !eligible).with("fallback_reasons", Detail::texts(reasons))
        .text("obligation", "Preparation is not review. Examine required content and guidance; justify reuse for every candidate, retrieve named dependent context, or use one full owner review. Save the actual examined/reused inventory and rationale separately. Acknowledge only the original full packet.")
        .with("readme", get(&content, "readme")).with("files", Detail::List(files))
        .with("imports", get(&content, "imports")).with("guidance", get(&context, "guidance"))
        .with("changes", get(&context, "changes")).with("diffs", get(&context, "diffs"))
        .with("exports", get(&context, "exports")).with("consumers", get(&context, "consumers"))
        .with("invalidations", get(data, "covered_invalidations"))
        .with("prior_review", get(&context, "previous_review"))
        .with("coverage", Detail::list(std::iter::once(DetailMap::default()
            .text("kind", "document").text("path", &p.content.readme.path)
            .text("required_disposition", "examine").bool("reviewed_by_this_view", false).build())
            .chain(p.content.files.iter().map(|f| DetailMap::default()
            .text("kind", "file").text("path", &f.path).text("hash", f.hash.to_hex()).number("bytes", f.bytes)
            .text("required_disposition", if eligible && !changed(&f.path) { "justify_reuse_or_examine" } else { "examine" })
            .bool("reviewed_by_this_view", false).build()))
            .chain(p.content.imports.iter().map(|i| DetailMap::default()
                .text("kind", "import").text("path", format!("{}#{}", i.document, i.export_id))
                .text("hash", i.hash.to_hex()).number("bytes", i.bytes)
                .text("required_disposition", "examine").bool("reviewed_by_this_view", false).build()))))
        .build()
}
