//! The shared snapshot pipeline used by every command.
//!
//! Collection gathers exact bytes and facts through the ports twice and
//! retries once when the two passes differ. Analysis then derives selection,
//! ownership, documents, graphs, policies, manifests, and review status from
//! one immutable set of facts.

use std::collections::{BTreeMap, BTreeSet};

use memoria_domain::canonical;
use memoria_domain::{
    ByteRange, DirPath, Document, DocumentId, DocumentStatus, EffectivePolicy, Exclusion, ExportId,
    FileInput, GitContext, GitRuleScope, Glob, GraphError, Hash64, Import, ImportGraph,
    ImportInput, InputManifest, NavigationGraph, OwnershipTree, PolicyRuleScope, ProjectPath,
    ReviewState, RuleScope, SelectionDecision,
};

use crate::config::RootConfig;
use crate::error::{AppError, Detail, DetailMap, Diagnostic, ExitClass, sort_diagnostics};
use crate::gitignore;
use crate::ports::{FileKind, LoadedState, Services, StateFailure};

pub const ROOT_CONFIG_PATH: &str = "memoria.yml";
pub const SIDECAR_FILE_NAME: &str = "README.memoria.yml";
pub const STATE_PATH: &str = ".memoria/state.json";
pub const STATE_DIR: &str = ".memoria";

/// A writing instruction applicable within a configuration scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub scope: DirPath,
    /// `memoria.yml`, a sidecar path, or an instruction file path.
    pub source: String,
    /// `inline` or `file`.
    pub kind: &'static str,
    pub text: String,
}

/// One configuration scope: the root or a sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeConfig {
    pub dir: DirPath,
    pub source: String,
    pub rules: RuleScope,
    pub policy: PolicyRuleScope,
    pub instructions: Vec<Instruction>,
    pub instruction_files: Vec<ProjectPath>,
}

/// Exact facts gathered from the ports in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collected {
    pub root_config: RootConfig,
    pub scopes: Vec<ScopeConfig>,
    pub eligible: Vec<ProjectPath>,
    pub kinds: BTreeMap<ProjectPath, FileKind>,
    pub boundaries: Vec<ProjectPath>,
    pub documents: BTreeSet<DocumentId>,
    pub decisions: BTreeMap<ProjectPath, SelectionDecision>,
    pub file_bytes: BTreeMap<ProjectPath, Vec<u8>>,
    pub document_bytes: BTreeMap<DocumentId, Vec<u8>>,
    pub instruction_bytes: BTreeMap<ProjectPath, Vec<u8>>,
    pub gitignores: BTreeMap<ProjectPath, Vec<u8>>,
    pub global_excludes: Option<Vec<u8>>,
    pub repository_excludes: Option<Vec<u8>>,
    pub state: Option<LoadedState>,
    pub diagnostics: Vec<Diagnostic>,
}

/// One import copy that no longer matches its provider's export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutdatedImport {
    pub document: DocumentId,
    pub provider: DocumentId,
    pub export_id: ExportId,
    pub body: ByteRange,
    pub location: memoria_domain::SourceLocation,
}

/// Everything derived from one collected pass.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub collected: Collected,
    pub documents: BTreeMap<DocumentId, Document>,
    pub ownership: OwnershipTree,
    pub graph: Option<ImportGraph>,
    pub navigation: NavigationGraph,
    pub disconnected: Vec<DocumentId>,
    pub policies: BTreeMap<DocumentId, EffectivePolicy>,
    pub policy_hashes: BTreeMap<DocumentId, Hash64>,
    pub manifests: BTreeMap<DocumentId, InputManifest>,
    pub state: ReviewState,
    pub statuses: Vec<DocumentStatus>,
    pub outdated_imports: Vec<OutdatedImport>,
    pub git: GitContext,
    pub diagnostics: Vec<Diagnostic>,
}

/// Build a stable snapshot: two collection passes plus one retry.
pub fn build(services: &Services<'_>) -> Result<Snapshot, AppError> {
    let first = collect(services)?;
    let second = collect(services)?;
    let stable = if first == second {
        second
    } else {
        let third = collect(services)?;
        if second != third {
            return Err(AppError::conflict(
                "snapshot_changed",
                "project inputs changed repeatedly while scanning; retry when the worktree is quiet",
            ));
        }
        third
    };
    Ok(analyze(services, stable))
}

/// Directory name of every managed skill package.
pub const MANAGED_PACKAGE_NAME: &str = "memoria";
/// Installation record inside a managed skill package.
pub const MANAGED_RECORD_FILE: &str = ".memoria-install.json";
/// Durable transaction record kept beside a managed package during a mutation.
pub const MANAGED_TRANSACTION_FILE: &str = "memoria.install-txn.json";

/// Whether `path` lies inside a managed package or one of its transaction
/// artifacts (`memoria.backup*`, `memoria.staging`, `memoria.removing`,
/// `memoria.install-txn.json`, `memoria.install.lock`) under `parent`.
fn within_managed_package(path: &ProjectPath, parent: &DirPath) -> bool {
    path.strip_dir(parent)
        .and_then(|relative| relative.split('/').next())
        .is_some_and(is_managed_artifact)
}

/// Exact identities the installer creates beside a package: the package
/// itself, staging and removal directories, the transaction record, the
/// lock, and numbered backups. Other `memoria.*` names are ordinary sources.
pub fn is_managed_artifact(name: &str) -> bool {
    name == MANAGED_PACKAGE_NAME
        || name == "memoria.staging"
        || name == "memoria.removing"
        || name == MANAGED_TRANSACTION_FILE
        || name == "memoria.install.lock"
        || name == "memoria.backup"
        || name
            .strip_prefix("memoria.backup-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether a path lies in a tree the tool itself owns: Git metadata, the
/// Memoria state directory, or a managed guidance package with its
/// transaction artifacts and backups. Such trees hold no documentation
/// boundaries and no source inputs.
fn tool_reserved(path: &ProjectPath, managed_parents: &BTreeSet<DirPath>) -> bool {
    let mut components = path.components();
    let first = components.next().unwrap_or("");
    if first == ".git" || first == STATE_DIR {
        return true;
    }
    if (first == ".agents" || first == ".claude")
        && components.next() == Some("skills")
        && components.next().is_some_and(is_managed_artifact)
    {
        return true;
    }
    if path.file_name() == "memoria.install.lock" {
        return true;
    }
    managed_parents
        .iter()
        .any(|parent| within_managed_package(path, parent))
}

fn reserved_category(
    path: &ProjectPath,
    instruction_files: &BTreeSet<ProjectPath>,
    managed_parents: &BTreeSet<DirPath>,
) -> Option<&'static str> {
    let mut components = path.components();
    let first = components.next().unwrap_or("");
    let file_name = path.file_name();
    if first == ".git" {
        return Some("git-metadata");
    }
    if first == STATE_DIR {
        return Some("state");
    }
    if path.as_str() == ROOT_CONFIG_PATH {
        return Some("configuration");
    }
    if file_name == SIDECAR_FILE_NAME {
        return Some("sidecar");
    }
    if file_name == ".gitignore" {
        return Some("gitignore");
    }
    if path.is_readme() {
        return Some("document");
    }
    if instruction_files.contains(path) {
        return Some("instruction-file");
    }
    if (first == ".agents" || first == ".claude")
        && components.next() == Some("skills")
        && components.next().is_some_and(is_managed_artifact)
    {
        return Some("skill-package");
    }
    if managed_parents
        .iter()
        .any(|parent| within_managed_package(path, parent))
    {
        return Some("skill-package");
    }
    // The installer's parent lock is never deleted; its exact name is a
    // recognized artifact even after the package itself was removed.
    if file_name == "memoria.install.lock" {
        return Some("skill-package");
    }
    None
}

/// The nearest ancestor directory (below the root) that contains a `.git`
/// entry, as a boundary path.
fn nested_boundary(
    services: &Services<'_>,
    path: &ProjectPath,
    cache: &mut BTreeMap<DirPath, bool>,
) -> Option<ProjectPath> {
    for dir in path.directory().ancestors() {
        if dir.is_root() {
            break;
        }
        let nested = *cache
            .entry(dir.clone())
            .or_insert_with(|| services.files.has_git_entry(dir.as_str()));
        if nested {
            return ProjectPath::parse(dir.as_str()).ok();
        }
    }
    None
}

fn git_error(err: crate::ports::AdapterError) -> AppError {
    AppError::io("git_unavailable", err.to_string())
}

fn io_error(err: crate::ports::AdapterError) -> AppError {
    AppError::io("io_error", err.to_string())
}

/// Structural validation plus recomputed fingerprints for loaded state.
pub fn validate_state(services: &Services<'_>, state: &ReviewState) -> Result<(), AppError> {
    if let Err(err) = state.validate() {
        return Err(AppError::new(
            ExitClass::Io,
            Diagnostic::error("state_corrupt", err.to_string()).at_path(STATE_PATH),
        ));
    }
    for (document, record) in &state.reviews {
        let expected = services
            .hasher
            .hash(&canonical::encode_inputs(&record.manifest));
        if expected != record.input_fingerprint {
            return Err(AppError::new(
                ExitClass::Io,
                Diagnostic::error(
                    "state_corrupt",
                    format!("stored input_fingerprint for {document} does not match its manifest"),
                )
                .at_path(STATE_PATH),
            ));
        }
    }
    Ok(())
}

/// Read and validate the root configuration; missing is a validation error.
pub fn read_root_config(services: &Services<'_>) -> Result<RootConfig, AppError> {
    match services.files.kind(ROOT_CONFIG_PATH).map_err(io_error)? {
        FileKind::Missing => Err(AppError::validation(
            "configuration_missing",
            format!("{ROOT_CONFIG_PATH} does not exist; run `memoria init` first"),
        )
        .with_diagnostics(vec![])),
        FileKind::Regular => {
            let bytes = services.files.read(ROOT_CONFIG_PATH).map_err(io_error)?;
            services.config.parse_root(&bytes).map_err(|message| {
                AppError::new(
                    ExitClass::Validation,
                    Diagnostic::error("configuration_invalid", message).at_path(ROOT_CONFIG_PATH),
                )
            })
        }
        other => Err(AppError::validation(
            "configuration_invalid",
            format!("{ROOT_CONFIG_PATH} must be a regular file, found {other:?}"),
        )),
    }
}

fn compile_globs(
    patterns: &[String],
    source: &str,
    kind: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Glob> {
    let mut globs = Vec::new();
    for pattern in patterns {
        match Glob::parse(pattern) {
            Ok(glob) => globs.push(glob),
            Err(err) => diagnostics.push(
                Diagnostic::error("configuration_invalid", format!("{kind} pattern: {err}"))
                    .at_path(source),
            ),
        }
    }
    globs
}

fn collect(services: &Services<'_>) -> Result<Collected, AppError> {
    let root_config = read_root_config(services)?;
    let mut diagnostics = Vec::new();

    if let Ok(true) = services.git.sparse_checkout_enabled() {
        return Err(AppError::io(
            "git_unsupported",
            "sparse checkouts are not supported",
        ));
    }
    let unmerged = services.git.unmerged_paths().map_err(git_error)?;
    if !unmerged.is_empty() {
        return Err(AppError::io(
            "git_unsupported",
            format!("unmerged index entries: {}", unmerged.join(", ")),
        ));
    }

    // Eligible paths.
    let mut eligible: Vec<ProjectPath> = Vec::new();
    let mut boundaries: Vec<ProjectPath> = Vec::new();
    let mut kinds: BTreeMap<ProjectPath, FileKind> = BTreeMap::new();
    let mut seen: BTreeSet<ProjectPath> = BTreeSet::new();
    let mut boundary_cache: BTreeMap<DirPath, bool> = BTreeMap::new();
    for raw in services.git.eligible_paths().map_err(git_error)? {
        let Ok(text) = std::str::from_utf8(&raw) else {
            diagnostics.push(Diagnostic::error(
                "path_invalid",
                format!(
                    "path {:?} is not valid UTF-8",
                    String::from_utf8_lossy(&raw)
                ),
            ));
            continue;
        };
        let is_dir_entry = text.ends_with('/');
        let path = match ProjectPath::parse(text.trim_end_matches('/')) {
            Ok(path) => path,
            Err(err) => {
                diagnostics.push(Diagnostic::error("path_invalid", err.to_string()).at_path(text));
                continue;
            }
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        // A `.git` entry in any ancestor directory below the root marks a
        // nested repository or submodule. Its contents are opaque even when
        // the parent index still tracks them.
        if let Some(boundary) = nested_boundary(services, &path, &mut boundary_cache) {
            boundaries.push(boundary);
            continue;
        }
        let kind = services.files.kind(path.as_str()).map_err(io_error)?;
        if is_dir_entry || kind == FileKind::Directory {
            boundaries.push(path);
            continue;
        }
        kinds.insert(path.clone(), kind);
        eligible.push(path);
    }
    eligible.sort();
    boundaries.sort();
    boundaries.dedup();

    // Validated managed skill packages at custom in-project locations are
    // guidance, not sources. Their transaction artifacts sit beside them.
    let mut managed_parents: BTreeSet<DirPath> = BTreeSet::new();
    for path in &eligible {
        if path.file_name() == MANAGED_TRANSACTION_FILE {
            // An interrupted installer transaction keeps its artifacts reserved
            // until recovery, so a crash never changes review inputs.
            managed_parents.insert(path.directory());
            continue;
        }
        if path.file_name() != MANAGED_RECORD_FILE {
            continue;
        }
        let package = path.directory();
        let is_package_dir = package.as_str().rsplit('/').next() == Some(MANAGED_PACKAGE_NAME);
        if is_package_dir
            && let Some(parent) = package.parent()
            && services.skills.is_managed_package(package.as_str())
        {
            managed_parents.insert(parent);
        }
    }

    // Documents and sidecars. Tool-reserved trees (Git metadata, Memoria
    // state, installed guidance packages and their backups) never hold
    // project documentation boundaries.
    let mut documents: BTreeSet<DocumentId> = BTreeSet::new();
    let mut sidecar_paths: Vec<ProjectPath> = Vec::new();
    for path in &eligible {
        if tool_reserved(path, &managed_parents) {
            continue;
        }
        if path.is_readme() {
            match kinds[path] {
                FileKind::Regular => {
                    documents.insert(DocumentId::from_path(path.clone()).expect("readme"));
                }
                FileKind::Missing => {
                    // Tracked but deleted from the worktree: no boundary today.
                }
                other => diagnostics.push(
                    Diagnostic::error(
                        "path_unsupported",
                        format!("README must be a regular file, found {other:?}"),
                    )
                    .at_path(path.as_str()),
                ),
            }
        } else if path.file_name() == SIDECAR_FILE_NAME {
            sidecar_paths.push(path.clone());
        }
    }

    // Configuration scopes.
    let mut scopes: Vec<ScopeConfig> = Vec::new();
    let mut instruction_files: BTreeSet<ProjectPath> = BTreeSet::new();
    let root_scope = build_scope(
        DirPath::root(),
        ROOT_CONFIG_PATH,
        &root_config.ignore,
        &root_config.include,
        &root_config.instructions,
        &root_config.instruction_files,
        &mut diagnostics,
    );
    instruction_files.extend(root_scope.instruction_files.iter().cloned());
    scopes.push(root_scope);
    for sidecar in &sidecar_paths {
        let dir = sidecar.directory();
        if kinds[sidecar] == FileKind::Missing {
            // Tracked but deleted from the worktree: no local rules today.
            continue;
        }
        if !documents.contains(&dir.readme()) {
            diagnostics.push(
                Diagnostic::error(
                    "sidecar_orphan",
                    "README.memoria.yml has no sibling README.md",
                )
                .at_path(sidecar.as_str()),
            );
            continue;
        }
        if kinds[sidecar] != FileKind::Regular {
            diagnostics.push(
                Diagnostic::error("path_unsupported", "sidecar must be a regular file")
                    .at_path(sidecar.as_str()),
            );
            continue;
        }
        let bytes = services.files.read(sidecar.as_str()).map_err(io_error)?;
        match services.config.parse_sidecar(&bytes) {
            Ok(config) => {
                let scope = build_scope(
                    dir,
                    sidecar.as_str(),
                    &config.ignore,
                    &config.include,
                    &config.instructions,
                    &config.instruction_files,
                    &mut diagnostics,
                );
                instruction_files.extend(scope.instruction_files.iter().cloned());
                scopes.push(scope);
            }
            Err(message) => diagnostics
                .push(Diagnostic::error("sidecar_invalid", message).at_path(sidecar.as_str())),
        }
    }
    scopes.sort_by_key(|scope| (scope.dir.depth(), scope.dir.clone()));

    // Instruction files.
    let mut instruction_bytes: BTreeMap<ProjectPath, Vec<u8>> = BTreeMap::new();
    for path in &instruction_files {
        if let Some(bytes) = read_instruction_file(services, path, &mut diagnostics)? {
            instruction_bytes.insert(path.clone(), bytes);
        }
    }

    // Selection.
    let rule_scopes: Vec<RuleScope> = scopes.iter().map(|s| s.rules.clone()).collect();
    let mut decisions: BTreeMap<ProjectPath, SelectionDecision> = BTreeMap::new();
    let mut file_bytes: BTreeMap<ProjectPath, Vec<u8>> = BTreeMap::new();
    let mut document_bytes: BTreeMap<DocumentId, Vec<u8>> = BTreeMap::new();
    let mut gitignores: BTreeMap<ProjectPath, Vec<u8>> = BTreeMap::new();
    for path in &eligible {
        let reserved =
            reserved_category(path, &instruction_files, &managed_parents).map(str::to_string);
        let decision = memoria_domain::selection::decide(path, reserved, &rule_scopes);
        if decision.selected() {
            match kinds[path] {
                FileKind::Regular => {
                    let bytes = services.files.read(path.as_str()).map_err(io_error)?;
                    file_bytes.insert(path.clone(), bytes);
                }
                FileKind::Symlink => diagnostics.push(
                    Diagnostic::error("path_unsupported", "selected symlinks are not supported; exclude the path with a Memoria ignore rule")
                        .at_path(path.as_str()),
                ),
                FileKind::Missing => {
                    // Tracked but deleted from the worktree: absent from inputs.
                }
                other => diagnostics.push(
                    Diagnostic::error("path_unsupported", format!("selected path is an unsupported {other:?}")).at_path(path.as_str()),
                ),
            }
        }
        decisions.insert(path.clone(), decision);
    }
    // Git applies a `.gitignore` in every directory it traverses, even when
    // that file is itself ignored or untracked and even when the directory
    // holds no eligible file. Walk the directories Git traverses: skip
    // `.git`, directories that Git's ignore rules exclude as directories,
    // and nested repositories. Ignored trees are never entered.
    // Only directories Git actually traverses for ignore selection carry
    // active `.gitignore` scopes. A tracked descendant inside an ignored
    // directory does not activate that directory's rules, and tool-reserved
    // trees (Git metadata, Memoria state, managed guidance and its backups)
    // never contribute policy.
    let mut policy_dirs: BTreeSet<DirPath> = BTreeSet::new();
    policy_dirs.insert(DirPath::root());
    let mut frontier: Vec<DirPath> = vec![DirPath::root()];
    while !frontier.is_empty() {
        let mut candidates: Vec<(DirPath, String)> = Vec::new();
        for dir in frontier.drain(..) {
            for name in services
                .files
                .subdirectories(dir.as_str())
                .map_err(io_error)?
            {
                if name == ".git" || name == STATE_DIR && dir.is_root() {
                    continue;
                }
                let Ok(child) =
                    DirPath::parse(format!("{}/{name}", dir.as_str()).trim_start_matches('/'))
                else {
                    continue;
                };
                if services.files.has_git_entry(child.as_str()) {
                    continue;
                }
                if let Ok(probe) = child.join(".gitignore")
                    && tool_reserved(&probe, &managed_parents)
                {
                    continue;
                }
                candidates.push((child.clone(), child.as_str().to_string()));
            }
        }
        if candidates.is_empty() {
            break;
        }
        let names: Vec<String> = candidates.iter().map(|(_, name)| name.clone()).collect();
        let ignored: BTreeSet<String> = services
            .git
            .ignored_directories(&names)
            .map_err(git_error)?
            .into_iter()
            .collect();
        for (dir, name) in candidates {
            if ignored.contains(&name) {
                continue;
            }
            policy_dirs.insert(dir.clone());
            frontier.push(dir);
        }
    }
    for dir in &policy_dirs {
        let Ok(candidate) = dir.join(".gitignore") else {
            continue;
        };
        if services.files.kind(candidate.as_str()).map_err(io_error)? == FileKind::Regular {
            gitignores.insert(
                candidate.clone(),
                services.files.read(candidate.as_str()).map_err(io_error)?,
            );
        }
    }
    for document in &documents {
        document_bytes.insert(
            document.clone(),
            services.files.read(document.as_str()).map_err(io_error)?,
        );
    }

    let global_excludes = services.git.global_excludes().map_err(git_error)?;
    let repository_excludes = services.git.repository_excludes().map_err(git_error)?;

    let state = match services.state.load() {
        Ok(state) => state,
        Err(StateFailure::Io(err)) => {
            return Err(AppError::io("state_unreadable", err.to_string()));
        }
        Err(StateFailure::Corrupt(message)) => {
            return Err(AppError::new(
                ExitClass::Io,
                Diagnostic::error("state_corrupt", message).at_path(STATE_PATH),
            ));
        }
        Err(StateFailure::Conflict) => {
            return Err(AppError::conflict(
                "state_conflict",
                "state changed while loading",
            ));
        }
    };
    if let Some(loaded) = &state {
        validate_state(services, &loaded.state)?;
    }

    Ok(Collected {
        root_config,
        scopes,
        eligible,
        kinds,
        boundaries,
        documents,
        decisions,
        file_bytes,
        document_bytes,
        instruction_bytes,
        gitignores,
        global_excludes,
        repository_excludes,
        state,
        diagnostics,
    })
}

/// Read one configured instruction file with the inspection rules: it must
/// exist, be a regular file (never a symlink), and be UTF-8. Problems are
/// reported as diagnostics; `Ok(None)` means the file could not be used.
fn read_instruction_file(
    services: &Services<'_>,
    path: &ProjectPath,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<Vec<u8>>, AppError> {
    match services.files.kind(path.as_str()).map_err(io_error)? {
        FileKind::Regular => {
            let bytes = services.files.read(path.as_str()).map_err(io_error)?;
            if std::str::from_utf8(&bytes).is_err() {
                diagnostics.push(
                    Diagnostic::error(
                        "instruction_file_invalid",
                        "instruction file is not valid UTF-8",
                    )
                    .at_path(path.as_str()),
                );
            }
            Ok(Some(bytes))
        }
        FileKind::Missing => {
            diagnostics.push(
                Diagnostic::error(
                    "instruction_file_missing",
                    "referenced instruction file does not exist",
                )
                .at_path(path.as_str()),
            );
            Ok(None)
        }
        other => {
            diagnostics.push(
                Diagnostic::error(
                    "instruction_file_invalid",
                    format!("instruction file must be a regular file, found {other:?}"),
                )
                .at_path(path.as_str()),
            );
            Ok(None)
        }
    }
}

/// Validate the semantics of an existing root configuration with the same
/// rules inspection applies: every ignore/include pattern must compile and
/// every configured instruction file must resolve inside the project to a
/// readable UTF-8 regular file. Used by `init` before any write.
pub fn validate_root_config(
    services: &Services<'_>,
    config: &RootConfig,
) -> Result<Vec<Diagnostic>, AppError> {
    let mut diagnostics = Vec::new();
    let scope = build_scope(
        DirPath::root(),
        ROOT_CONFIG_PATH,
        &config.ignore,
        &config.include,
        &config.instructions,
        &config.instruction_files,
        &mut diagnostics,
    );
    for path in &scope.instruction_files {
        read_instruction_file(services, path, &mut diagnostics)?;
    }
    sort_diagnostics(&mut diagnostics);
    Ok(diagnostics)
}

#[allow(clippy::too_many_arguments)]
fn build_scope(
    dir: DirPath,
    source: &str,
    ignore: &[String],
    include: &[String],
    instructions: &[String],
    instruction_files: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) -> ScopeConfig {
    let ignore_globs = compile_globs(ignore, source, "ignore", diagnostics);
    let include_globs = compile_globs(include, source, "include", diagnostics);
    let mut files = Vec::new();
    for relative in instruction_files {
        match ProjectPath::resolve_relative(&dir, relative) {
            Ok(path) => files.push(path),
            Err(err) => diagnostics.push(
                Diagnostic::error(
                    "instruction_file_invalid",
                    format!("instruction_files entry {relative:?}: {err}"),
                )
                .at_path(source),
            ),
        }
    }
    let inline: Vec<Instruction> = instructions
        .iter()
        .map(|text| Instruction {
            scope: dir.clone(),
            source: source.to_string(),
            kind: "inline",
            text: text.clone(),
        })
        .collect();
    ScopeConfig {
        dir: dir.clone(),
        source: source.to_string(),
        rules: RuleScope {
            scope: dir.clone(),
            ignore: ignore_globs,
            include: include_globs,
        },
        policy: PolicyRuleScope::new(dir, ignore.to_vec(), include.to_vec()),
        instructions: inline,
        instruction_files: files,
    }
}

fn analyze(services: &Services<'_>, collected: Collected) -> Snapshot {
    let mut diagnostics = collected.diagnostics.clone();
    let hasher = services.hasher;

    // Ownership.
    let selected: Vec<ProjectPath> = collected.file_bytes.keys().cloned().collect();
    let ownership = OwnershipTree::build(&collected.documents, &selected);
    let root_document = DirPath::root().readme();
    if !collected.documents.contains(&root_document) {
        diagnostics.push(
            Diagnostic::error("root_readme_missing", "the project root has no README.md")
                .at_path("README.md"),
        );
    }
    for path in ownership.unowned() {
        diagnostics.push(
            Diagnostic::error("coverage_unowned", "selected file has no README owner")
                .at_path(path.as_str()),
        );
    }

    // Documents.
    let mut documents: BTreeMap<DocumentId, Document> = BTreeMap::new();
    for document in &collected.documents {
        let bytes = &collected.document_bytes[document];
        let parsed = services.markdown.parse(document, bytes);
        let mut invalid = false;
        for issue in &parsed.issues {
            invalid = true;
            let mut diagnostic =
                Diagnostic::error(issue.code, issue.message.clone()).at_path(document.as_str());
            if let Some(location) = issue.location {
                diagnostic = diagnostic.at(location.line, location.column);
            }
            diagnostics.push(diagnostic);
        }
        let mut imports = Vec::new();
        for import in &parsed.imports {
            match resolve_import(document, &import.source_text) {
                Ok((provider, export_id)) => imports.push(Import {
                    provider,
                    export_id,
                    source_text: import.source_text.clone(),
                    body: import.body,
                    location: import.location,
                }),
                Err(message) => {
                    invalid = true;
                    diagnostics.push(
                        Diagnostic::error("import_invalid", message)
                            .at_path(document.as_str())
                            .at(import.location.line, import.location.column),
                    );
                }
            }
        }
        let links = resolve_links(document, &parsed.links, &collected.documents);
        if invalid {
            // Keep the document visible with whatever parsed cleanly.
        }
        documents.insert(
            document.clone(),
            Document {
                id: document.clone(),
                exports: parsed.exports,
                imports,
                links,
            },
        );
    }

    // Graph.
    let graph = match ImportGraph::build(&documents) {
        Ok(graph) => Some(graph),
        Err(errors) => {
            for error in errors {
                diagnostics.push(graph_diagnostic(error));
            }
            None
        }
    };
    let navigation = NavigationGraph::build(&documents);
    let disconnected = if collected.documents.contains(&root_document) {
        navigation.disconnected(&root_document)
    } else {
        Vec::new()
    };
    for document in &disconnected {
        diagnostics.push(
            Diagnostic::warning(
                "navigation_disconnected",
                "no link or import path from the root README reaches this README",
            )
            .at_path(document.as_str())
            .with_details(
                DetailMap::default()
                    .number("owned_files", ownership.owned_by(document).len() as u64)
                    .build(),
            ),
        );
    }
    if collected.root_config.missing_import_hint {
        for (id, document) in &documents {
            for link in &document.links {
                if !document
                    .imports
                    .iter()
                    .any(|import| &import.provider == link)
                {
                    diagnostics.push(
                        Diagnostic::hint(
                            "missing_import_hint",
                            format!("normal link to {link} has no matching import; add an import if its summary belongs here"),
                        )
                        .at_path(id.as_str())
                        .with_details(DetailMap::default().text("target", link.as_str()).build()),
                    );
                }
            }
        }
    }

    // Policies and manifests.
    let mut policies = BTreeMap::new();
    let mut policy_hashes = BTreeMap::new();
    let mut manifests = BTreeMap::new();
    for document in &collected.documents {
        let policy = effective_policy(&collected, &ownership, document);
        let policy_hash = hasher.hash(&canonical::encode_policy(&policy));
        policies.insert(document.clone(), policy);
        policy_hashes.insert(document.clone(), policy_hash);
    }
    if graph.is_some() {
        for document in &collected.documents {
            let bytes = &collected.document_bytes[document];
            let files: Vec<FileInput> = ownership
                .owned_by(document)
                .iter()
                .map(|path| {
                    let content = &collected.file_bytes[path];
                    FileInput {
                        path: path.clone(),
                        bytes: content.len() as u64,
                        hash: hasher.hash(content),
                    }
                })
                .collect();
            let imports: Vec<ImportInput> = documents[document]
                .imports
                .iter()
                .filter_map(|import| {
                    let body = export_body(
                        &documents,
                        &collected.document_bytes,
                        &import.provider,
                        &import.export_id,
                    )?;
                    Some(ImportInput {
                        document: import.provider.clone(),
                        export_id: import.export_id.clone(),
                        bytes: body.len() as u64,
                        hash: hasher.hash(body),
                    })
                })
                .collect();
            match InputManifest::new(
                document.clone(),
                policy_hashes[document],
                bytes.len() as u64,
                hasher.hash(bytes),
                files,
                imports,
            ) {
                Ok(manifest) => {
                    manifests.insert(document.clone(), manifest);
                }
                Err(err) => diagnostics.push(
                    Diagnostic::error("manifest_invalid", err.to_string())
                        .at_path(document.as_str()),
                ),
            }
        }
    }

    // Outdated imports.
    let mut outdated_imports = Vec::new();
    for (id, document) in &documents {
        let bytes = &collected.document_bytes[id];
        for import in &document.imports {
            let Some(export) = export_body(
                &documents,
                &collected.document_bytes,
                &import.provider,
                &import.export_id,
            ) else {
                continue;
            };
            let current = &bytes[import.body.start..import.body.end];
            if current != export {
                outdated_imports.push(OutdatedImport {
                    document: id.clone(),
                    provider: import.provider.clone(),
                    export_id: import.export_id.clone(),
                    body: import.body,
                    location: import.location,
                });
                diagnostics.push(
                    Diagnostic::warning(
                        "imports_outdated",
                        format!(
                            "import of {}#{} differs from the current export; run `memoria render`",
                            import.provider, import.export_id
                        ),
                    )
                    .at_path(id.as_str())
                    .at(import.location.line, import.location.column),
                );
            }
        }
    }

    // State and scheduling.
    let state = collected
        .state
        .as_ref()
        .map(|s| s.state.clone())
        .unwrap_or_default();
    let statuses = match &graph {
        Some(graph) => memoria_domain::schedule(graph, &state, &manifests),
        None => Vec::new(),
    };

    let git = GitContext {
        base_commit: services.git.head_commit().ok().flatten(),
        worktree_dirty: services.git.worktree_dirty().unwrap_or(true),
    };

    sort_diagnostics(&mut diagnostics);
    Snapshot {
        collected,
        documents,
        ownership,
        graph,
        navigation,
        disconnected,
        policies,
        policy_hashes,
        manifests,
        state,
        statuses,
        outdated_imports,
        git,
        diagnostics,
    }
}

fn graph_diagnostic(error: GraphError) -> Diagnostic {
    match &error {
        GraphError::MissingDocument {
            importer,
            location,
            target,
            ..
        } => Diagnostic::error("import_missing_document", error.to_string())
            .at_path(importer.as_str())
            .at(location.line, location.column)
            .with_details(DetailMap::default().text("target", target.as_str()).build()),
        GraphError::MissingExport {
            importer,
            location,
            provider,
            export_id,
        } => Diagnostic::error("import_missing_export", error.to_string())
            .at_path(importer.as_str())
            .at(location.line, location.column)
            .with_details(
                DetailMap::default()
                    .text("target", provider.as_str())
                    .text("export_id", export_id.as_str())
                    .build(),
            ),
        GraphError::SelfImport { importer, location } => {
            Diagnostic::error("import_self", error.to_string())
                .at_path(importer.as_str())
                .at(location.line, location.column)
        }
        GraphError::DuplicateImport {
            importer, location, ..
        } => Diagnostic::error("import_duplicate", error.to_string())
            .at_path(importer.as_str())
            .at(location.line, location.column),
        GraphError::Cycle { path, edges } => Diagnostic::error("import_cycle", error.to_string())
            .at_path(path[0].as_str())
            .with_details(
                DetailMap::default()
                    .with(
                        "path",
                        Detail::texts(path.iter().map(|d| d.as_str().to_string())),
                    )
                    .with(
                        "edges",
                        Detail::list(edges.iter().map(|edge| {
                            DetailMap::default()
                                .text("importer", edge.importer.as_str())
                                .text("provider", edge.provider.as_str())
                                .text("export_id", edge.export_id.as_str())
                                .number("line", edge.location.line as u64)
                                .number("column", edge.location.column as u64)
                                .build()
                        })),
                    )
                    .build(),
            ),
    }
}

/// Resolve `src="path/README.md#export"` relative to the importing document.
pub fn resolve_import(
    document: &DocumentId,
    source_text: &str,
) -> Result<(DocumentId, ExportId), String> {
    let mut parts = source_text.split('#');
    let path_part = parts.next().unwrap_or("");
    let Some(export_part) = parts.next() else {
        return Err(format!(
            "import src {source_text:?} must contain exactly one `#` followed by an export id"
        ));
    };
    if parts.next().is_some() {
        return Err(format!(
            "import src {source_text:?} must contain exactly one `#`"
        ));
    }
    let export_id = ExportId::parse(export_part).map_err(|e| e.to_string())?;
    let path = ProjectPath::resolve_relative(&document.directory(), path_part)
        .map_err(|e| e.to_string())?;
    let provider = DocumentId::from_path(path).map_err(|e| e.to_string())?;
    Ok((provider, export_id))
}

fn resolve_links(
    document: &DocumentId,
    links: &[String],
    documents: &BTreeSet<DocumentId>,
) -> Vec<DocumentId> {
    let mut out = Vec::new();
    for raw in links {
        let without_fragment = raw.split('#').next().unwrap_or("");
        let without_query = without_fragment.split('?').next().unwrap_or("");
        if without_query.is_empty()
            || without_query.contains("://")
            || without_query.starts_with("mailto:")
        {
            continue;
        }
        // URL path escapes are decoded exactly once after the query and
        // fragment are separated; an invalid encoding is not a local link.
        let Some(decoded) = percent_decode(without_query) else {
            continue;
        };
        let without_query = decoded.as_str();
        let candidate = if let Some(root_relative) = without_query.strip_prefix('/') {
            DirPath::root().join(root_relative.trim_end_matches('/'))
        } else if without_query == "." || without_query == "./" {
            Ok(document.directory().readme().path().clone())
        } else {
            ProjectPath::resolve_relative(
                &document.directory(),
                without_query.trim_end_matches('/'),
            )
        };
        let Ok(path) = candidate else { continue };
        let target = if let Ok(direct) = DocumentId::from_path(path.clone()) {
            direct
        } else {
            DirPath::parse(path.as_str())
                .map(|dir| dir.readme())
                .unwrap_or_else(|_| document.clone())
        };
        if documents.contains(&target) && &target != document && !out.contains(&target) {
            out.push(target);
        }
    }
    out
}

/// Decode `%XX` escapes once. `+` stays literal. Invalid or non-UTF-8
/// sequences yield `None`.
pub fn percent_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_string());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The current bytes of an export body.
pub fn export_body<'a>(
    documents: &BTreeMap<DocumentId, Document>,
    document_bytes: &'a BTreeMap<DocumentId, Vec<u8>>,
    provider: &DocumentId,
    export_id: &ExportId,
) -> Option<&'a [u8]> {
    let export = documents.get(provider)?.export(export_id)?;
    let bytes = document_bytes.get(provider)?;
    bytes.get(export.body.start..export.body.end)
}

fn effective_policy(
    collected: &Collected,
    ownership: &OwnershipTree,
    owner: &DocumentId,
) -> EffectivePolicy {
    let owner_dir = owner.directory();
    let ancestors = owner_dir.ancestors();
    let mut git_scopes = Vec::new();
    // A scope without effective rules is canonically the same as no scope:
    // comment-only or empty files carry no policy.
    let push_scope = |scopes: &mut Vec<GitRuleScope>, identity: &str, bytes: &[u8]| {
        let patterns = gitignore::effective_patterns(bytes);
        if !patterns.is_empty() {
            scopes.push(GitRuleScope {
                identity: identity.to_string(),
                patterns,
            });
        }
    };
    if let Some(bytes) = &collected.global_excludes {
        push_scope(&mut git_scopes, "global", bytes);
    }
    if let Some(bytes) = &collected.repository_excludes {
        push_scope(&mut git_scopes, "repository", bytes);
    }
    for (path, bytes) in &collected.gitignores {
        let dir = path.directory();
        let is_ancestor = ancestors.contains(&dir);
        let in_coverage = dir.is_within(&owner_dir)
            && nearest_document(&collected.documents, &dir).as_ref() == Some(owner);
        if is_ancestor || in_coverage {
            push_scope(&mut git_scopes, path.as_str(), bytes);
        }
    }
    // Writing instructions are review context only: a sidecar that declares
    // no ignore or include rule is not a selection-policy scope at any depth,
    // including a root-located sidecar. The `memoria.yml` configuration
    // scope always is, because it defines the effective policy.
    let memoria_scopes: Vec<PolicyRuleScope> = collected
        .scopes
        .iter()
        .filter(|scope| ancestors.contains(&scope.dir))
        .filter(|scope| {
            scope.source == ROOT_CONFIG_PATH
                || !scope.policy.ignore.is_empty()
                || !scope.policy.include.is_empty()
        })
        .map(|scope| scope.policy.clone())
        .collect();
    let _ = ownership;
    EffectivePolicy::new(owner.clone(), git_scopes, memoria_scopes)
}

fn nearest_document(documents: &BTreeSet<DocumentId>, dir: &DirPath) -> Option<DocumentId> {
    dir.ancestors()
        .iter()
        .map(DirPath::readme)
        .find(|candidate| documents.contains(candidate))
}

impl Snapshot {
    /// Structural errors that make review, render, and check impossible.
    pub fn structural_errors(&self) -> Vec<Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.is_error())
            .cloned()
            .collect()
    }

    pub fn non_error_diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| !d.is_error())
            .cloned()
            .collect()
    }

    /// Fail with every structural error when any exists.
    pub fn require_valid(&self) -> Result<(), AppError> {
        let errors = self.structural_errors();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AppError::many(ExitClass::Validation, errors))
        }
    }

    pub fn status_of(&self, document: &DocumentId) -> Option<&DocumentStatus> {
        self.statuses.iter().find(|s| &s.document == document)
    }

    pub fn document_bytes(&self, document: &DocumentId) -> &[u8] {
        self.collected
            .document_bytes
            .get(document)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn file_bytes(&self, path: &ProjectPath) -> &[u8] {
        self.collected
            .file_bytes
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn export_body(&self, provider: &DocumentId, export_id: &ExportId) -> Option<&[u8]> {
        export_body(
            &self.documents,
            &self.collected.document_bytes,
            provider,
            export_id,
        )
    }

    /// Instructions from the root to the nearest scope of a document.
    pub fn applicable_instructions(&self, document: &DocumentId) -> Vec<Instruction> {
        let ancestors = document.directory().ancestors();
        let mut out = Vec::new();
        for scope in &self.collected.scopes {
            if !ancestors.contains(&scope.dir) {
                continue;
            }
            out.extend(scope.instructions.iter().cloned());
            for path in &scope.instruction_files {
                if let Some(bytes) = self.collected.instruction_bytes.get(path) {
                    out.push(Instruction {
                        scope: scope.dir.clone(),
                        source: path.as_str().to_string(),
                        kind: "file",
                        text: String::from_utf8_lossy(bytes).into_owned(),
                    });
                }
            }
        }
        out
    }

    pub fn outdated_imports_of(&self, document: &DocumentId) -> Vec<&OutdatedImport> {
        self.outdated_imports
            .iter()
            .filter(|o| &o.document == document)
            .collect()
    }

    pub fn selected_bytes(&self) -> u64 {
        self.collected
            .file_bytes
            .values()
            .map(|b| b.len() as u64)
            .sum()
    }

    pub fn selection_exclusion_summary(&self) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        for decision in self.collected.decisions.values() {
            let key = match &decision.exclusion {
                None => continue,
                Some(Exclusion::Reserved(category)) => format!("reserved:{category}"),
                Some(Exclusion::Rule { .. }) => "memoria-rule".to_string(),
            };
            *counts.entry(key).or_insert(0) += 1;
        }
        if !self.collected.boundaries.is_empty() {
            counts.insert(
                "nested-repository".to_string(),
                self.collected.boundaries.len() as u64,
            );
        }
        counts
    }
}
