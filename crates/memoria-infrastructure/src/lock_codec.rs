//! The `memoria.lock` binary codec: format 2.
//!
//! The artifact is generated, machine-owned state. It is neither TOML nor a
//! process synchronization lock. One logical state always produces one
//! canonical byte sequence, so two hosts with equal review history commit
//! equal bytes.
//!
//! Layout: a fixed frame (`MML\0`, format version, codec, decoded length,
//! body, XXH3-128 checksum) around a normalized payload of eight sections.
//! Repeated strings, paths, content descriptors, guidance digests, Git
//! contexts, and integer vectors move into canonical tables, so a record
//! stores indexes instead of repeated bytes.
//!
//! The decoder is strict: it verifies the checksum before decompression,
//! bounds every allocation, and rejects noncanonical encodings that would
//! let two byte sequences mean the same state.

use std::collections::BTreeMap;

use memoria_domain::{
    DirPath, DocumentId, ExportId, FileInput, GitContext, GuidanceDigest, Hash64, ImportInput,
    InputManifest, Invalidation, InvalidationScope, ProjectPath, Reason, ReviewNote, ReviewRecord,
    ReviewResult, ReviewState, ReviewerName, Timestamp, canonical,
};

use crate::hash::{xxh3_64, xxh3_128};

/// `MML` followed by NUL. The NUL makes Git treat the artifact as binary.
pub const MAGIC: [u8; 4] = [0x4d, 0x4d, 0x4c, 0x00];
/// The only supported format version.
pub const FORMAT_VERSION: u8 = 2;
/// Raw payload.
pub const CODEC_RAW: u8 = 0;
/// One ordinary Zstandard frame under the fixed profile.
pub const CODEC_ZSTD: u8 = 1;

/// Encoded file limit, including header and checksum: 64 MiB.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Decoded payload limit: 64 MiB.
pub const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// Expanded scalar values: integers, tags, hashes, and string references.
pub const MAX_EXPANDED_VALUES: u64 = 1_000_000;
/// Expanded string bytes, counting every logical occurrence.
pub const MAX_EXPANDED_STRING_BYTES: u64 = 64 * 1024 * 1024;
/// Decoding window: 1 MiB.
pub const MAX_WINDOW_LOG: u32 = 20;

/// Codec names published by read-only inspection.
pub fn codec_name(codec: u8) -> &'static str {
    match codec {
        CODEC_ZSTD => "zstd-v1",
        _ => "raw",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Malformed framing, a failed checksum, or an impossible payload.
    Corrupt(String),
    /// A read, payload, or expansion limit would be exceeded.
    Limit(String),
    /// The format version is outside this release's support.
    UnsupportedSchema(String),
    /// The codec identifier is outside this release's support.
    UnsupportedCodec(String),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Corrupt(m)
            | LockError::Limit(m)
            | LockError::UnsupportedSchema(m)
            | LockError::UnsupportedCodec(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for LockError {}

fn corrupt<T>(message: impl Into<String>) -> Result<T, LockError> {
    Err(LockError::Corrupt(message.into()))
}

/// Framing facts plus the decoded state and its guidance digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedLock {
    pub file_bytes: u64,
    pub payload_bytes: u64,
    pub format_version: u8,
    pub codec: u8,
    /// XXH3-128 over every byte before the trailer, as 32 lowercase hex.
    pub checksum: String,
    pub state: ReviewState,
    /// The guidance digest each review recorded, by document.
    pub guidance: BTreeMap<DocumentId, GuidanceDigest>,
}

// ---------------------------------------------------------------- primitives

fn put_u(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_z(out: &mut Vec<u8>, value: i64) {
    let zigzag = ((value << 1) ^ (value >> 63)) as u64;
    put_u(out, zigzag);
}

fn put_h(out: &mut Vec<u8>, hash: Hash64) {
    out.extend_from_slice(&hash.0.to_be_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    budget: Budget,
}

/// The expansion budget of one payload.
///
/// The reader charges every materialized occurrence against this budget, and
/// the writer preflight charges the same occurrences from the typed state.
/// One type keeps the two sides on identical accounting rules.
#[derive(Debug, Default, Clone, Copy)]
struct Budget {
    values: u64,
    string_bytes: u64,
}

impl Budget {
    /// Charge `count` expanded scalar values.
    fn spend_values(&mut self, count: u64) -> Result<(), LockError> {
        self.values = self.values.saturating_add(count);
        if self.values > MAX_EXPANDED_VALUES {
            return Err(LockError::Limit(format!(
                "the payload expands to more than {MAX_EXPANDED_VALUES} scalar values"
            )));
        }
        Ok(())
    }

    /// Charge `count` expanded string bytes.
    fn spend_string_bytes(&mut self, count: u64) -> Result<(), LockError> {
        self.string_bytes = self.string_bytes.saturating_add(count);
        if self.string_bytes > MAX_EXPANDED_STRING_BYTES {
            return Err(LockError::Limit(format!(
                "the payload expands to more than {MAX_EXPANDED_STRING_BYTES} string bytes"
            )));
        }
        Ok(())
    }
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader {
            bytes,
            position: 0,
            budget: Budget::default(),
        }
    }

    fn spend_value(&mut self) -> Result<(), LockError> {
        self.spend_values(1)
    }

    /// Charge `count` expanded scalar values against the budget.
    fn spend_values(&mut self, count: u64) -> Result<(), LockError> {
        self.budget.spend_values(count)
    }

    fn spend_string_bytes(&mut self, count: u64) -> Result<(), LockError> {
        self.budget.spend_string_bytes(count)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], LockError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| LockError::Corrupt("a length overflows the payload".into()))?;
        if end > self.bytes.len() {
            return Err(LockError::Corrupt(format!(
                "the payload ends after {} bytes but a field needs {end}",
                self.bytes.len()
            )));
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    /// One shortest unsigned LEB128. Longer-than-necessary encodings,
    /// overflow, and values wider than `u64` are rejected.
    fn u(&mut self) -> Result<u64, LockError> {
        self.spend_value()?;
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *self
                .take(1)?
                .first()
                .expect("take(1) returns exactly one byte");
            let payload = u64::from(byte & 0x7f);
            if shift >= 64 || (shift == 63 && payload > 1) {
                return corrupt("an unsigned integer does not fit in 64 bits");
            }
            value |= payload << shift;
            if byte < 0x80 {
                // The shortest encoding never ends with a zero continuation
                // group, except for the single byte that encodes zero.
                if byte == 0 && shift > 0 {
                    return corrupt("an unsigned integer uses a longer encoding than necessary");
                }
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn z(&mut self) -> Result<i64, LockError> {
        let raw = self.u()?;
        Ok(((raw >> 1) as i64) ^ -((raw & 1) as i64))
    }

    fn h(&mut self) -> Result<Hash64, LockError> {
        self.spend_value()?;
        let bytes = self.take(8)?;
        Ok(Hash64(u64::from_be_bytes(
            bytes.try_into().expect("eight bytes"),
        )))
    }

    fn byte(&mut self) -> Result<u8, LockError> {
        self.spend_value()?;
        Ok(self.take(1)?[0])
    }

    fn finish(&self) -> Result<(), LockError> {
        if self.position != self.bytes.len() {
            return corrupt(format!(
                "the payload has {} unread trailing bytes",
                self.bytes.len() - self.position
            ));
        }
        Ok(())
    }
}

// ------------------------------------------------------------- logical model

/// The reviewed state plus the guidance digest of every review. The codec
/// keeps guidance beside the aggregate because the aggregate stores it as a
/// value and the table stores it once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPayload<'a> {
    pub state: &'a ReviewState,
}

/// The owner-relative path of a selected file.
fn relative_to_owner(document: &DocumentId, path: &ProjectPath) -> Result<String, LockError> {
    let owner = document.directory();
    match path.strip_dir(&owner) {
        Some(relative) if !relative.is_empty() => Ok(relative.to_string()),
        _ => Err(LockError::Corrupt(format!(
            "selected path {path} does not sit under the boundary that owns it ({document})"
        ))),
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` into UTC epoch seconds.
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' || bytes[19] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let slice = text.get(range)?;
        if !slice.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        slice.parse::<i64>().ok()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=9999).contains(&year) || !(1..=12).contains(&month) || day < 1 {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    if day > days_in_month(year, month) {
        return None;
    }
    Some((days_from_civil(year, month, day) * 86_400) + hour * 3600 + minute * 60 + second)
}

/// Render UTC epoch seconds as `YYYY-MM-DDTHH:MM:SSZ`, without any host
/// timezone dependence.
pub fn format_timestamp(epoch: i64) -> Option<String> {
    let days = epoch.div_euclid(86_400);
    let rest = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days)?;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    ))
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days from 1970-01-01 to the given proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> Option<(i64, i64, i64)> {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some((year, month, day))
}

fn git_wire(git: &GitContext) -> Result<Vec<u8>, LockError> {
    let mut flag: u8 = u8::from(git.worktree_dirty);
    let mut object = Vec::new();
    if let Some(commit) = &git.base_commit {
        let raw = decode_hex(commit).ok_or_else(|| {
            LockError::Corrupt(format!("git base_commit {commit:?} is not hexadecimal"))
        })?;
        match raw.len() {
            20 => flag |= 2,
            32 => flag |= 4,
            other => {
                return Err(LockError::Corrupt(format!(
                    "git base_commit has {other} bytes; only SHA-1 and SHA-256 identities fit"
                )));
            }
        }
        object = raw;
    }
    let mut out = vec![flag];
    out.extend_from_slice(&object);
    Ok(out)
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    if !bytes.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ------------------------------------------------------------------ encoding

/// The canonical tables of one state, in their fixed section order.
struct Tables {
    strings: Vec<String>,
    string_id: BTreeMap<String, usize>,
    paths: Vec<String>,
    path_id: BTreeMap<String, usize>,
    contents: Vec<(u64, Hash64)>,
    content_id: BTreeMap<(u64, Hash64), usize>,
    guidance: Vec<Hash64>,
    guidance_id: BTreeMap<Hash64, usize>,
    gits: Vec<Vec<u8>>,
    git_id: BTreeMap<Vec<u8>, usize>,
    vectors: Vec<Vec<u64>>,
    vector_id: BTreeMap<Vec<u64>, usize>,
    base_time: i64,
}

/// Sort key of the path trie: component depth, then complete path bytes.
fn path_key(path: &str) -> (usize, &str) {
    (path.matches('/').count() + 1, path)
}

fn build_tables(state: &ReviewState) -> Result<Tables, LockError> {
    use std::collections::{BTreeSet, HashMap};

    let mut strings: BTreeSet<String> = BTreeSet::new();
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let mut content_counts: HashMap<(u64, Hash64), u64> = HashMap::new();
    let mut gits: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut guidance: BTreeSet<Hash64> = BTreeSet::new();
    let mut times: Vec<i64> = Vec::new();

    let count = |descriptor: (u64, Hash64), counts: &mut HashMap<(u64, Hash64), u64>| {
        *counts.entry(descriptor).or_insert(0) += 1;
    };

    for (document, record) in &state.reviews {
        let manifest = &record.manifest;
        paths.insert(document.as_str().to_string());
        strings.insert(record.reviewer.as_str().to_string());
        strings.insert(record.note.as_str().to_string());
        count(
            (manifest.document_bytes, manifest.document_hash),
            &mut content_counts,
        );
        guidance.insert(record.guidance.0);
        gits.insert(git_wire(&record.git)?);
        times.push(timestamp_seconds(&record.reviewed_at)?);
        for file in manifest.files() {
            strings.insert(relative_to_owner(document, &file.path)?);
            count((file.bytes, file.hash), &mut content_counts);
        }
        for import in manifest.imports() {
            paths.insert(import.document.as_str().to_string());
            strings.insert(import.export_id.as_str().to_string());
            count((import.bytes, import.hash), &mut content_counts);
        }
    }
    for invalidation in &state.invalidations {
        strings.insert(invalidation.reason.as_str().to_string());
        times.push(timestamp_seconds(&invalidation.created_at)?);
        for document in invalidation.targets.iter().chain(&invalidation.pending) {
            paths.insert(document.as_str().to_string());
        }
        match &invalidation.scope {
            InvalidationScope::All => {}
            InvalidationScope::Document(document) => {
                paths.insert(document.as_str().to_string());
            }
            InvalidationScope::Subtree(dir) => {
                if !dir.is_root() {
                    paths.insert(dir.as_str().to_string());
                }
            }
        }
    }
    // Every required ancestor joins the trie, and every path component
    // joins the string table.
    for path in paths.clone() {
        let components: Vec<&str> = path.split('/').collect();
        for component in &components {
            strings.insert((*component).to_string());
        }
        for cut in 1..components.len() {
            paths.insert(components[..cut].join("/"));
        }
    }

    let mut string_list: Vec<String> = strings.into_iter().collect();
    string_list.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let string_id: BTreeMap<String, usize> = string_list
        .iter()
        .enumerate()
        .map(|(index, value)| (value.clone(), index))
        .collect();

    // Index 0 is the implicit empty root, which stores no row.
    let mut path_list: Vec<String> = paths.into_iter().collect();
    path_list.sort_by(|a, b| path_key(a).cmp(&path_key(b)));
    path_list.insert(0, String::new());
    let path_id: BTreeMap<String, usize> = path_list
        .iter()
        .enumerate()
        .map(|(index, value)| (value.clone(), index))
        .collect();

    let mut contents: Vec<(u64, Hash64)> = content_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(descriptor, _)| *descriptor)
        .collect();
    contents.sort();
    let content_id: BTreeMap<(u64, Hash64), usize> = contents
        .iter()
        .enumerate()
        .map(|(index, value)| (*value, index))
        .collect();

    let guidance_list: Vec<Hash64> = guidance.into_iter().collect();
    let guidance_id: BTreeMap<Hash64, usize> = guidance_list
        .iter()
        .enumerate()
        .map(|(index, value)| (*value, index))
        .collect();

    let git_list: Vec<Vec<u8>> = gits.into_iter().collect();
    let git_id: BTreeMap<Vec<u8>, usize> = git_list
        .iter()
        .enumerate()
        .map(|(index, value)| (value.clone(), index))
        .collect();

    let mut vector_set: BTreeSet<Vec<u64>> = BTreeSet::new();
    for record in state.reviews.values() {
        vector_set.insert(record.acknowledged_invalidations.clone());
    }
    for invalidation in &state.invalidations {
        for group in [&invalidation.targets, &invalidation.pending] {
            let mut ids: Vec<u64> = group
                .iter()
                .map(|document| path_id[document.as_str()] as u64)
                .collect();
            ids.sort_unstable();
            vector_set.insert(ids);
        }
    }
    let vectors: Vec<Vec<u64>> = vector_set.into_iter().collect();
    let vector_id: BTreeMap<Vec<u64>, usize> = vectors
        .iter()
        .enumerate()
        .map(|(index, value)| (value.clone(), index))
        .collect();

    Ok(Tables {
        strings: string_list,
        string_id,
        paths: path_list,
        path_id,
        contents,
        content_id,
        guidance: guidance_list,
        guidance_id,
        gits: git_list,
        git_id,
        vectors,
        vector_id,
        base_time: times.into_iter().min().unwrap_or(0),
    })
}

fn timestamp_seconds(stamp: &Timestamp) -> Result<i64, LockError> {
    parse_timestamp(&stamp.0).ok_or_else(|| {
        LockError::Corrupt(format!(
            "timestamp {:?} is not a Gregorian UTC second between 0001 and 9999",
            stamp.0
        ))
    })
}

/// Whether a state is the initial empty state, which has an empty payload.
fn is_initial(state: &ReviewState) -> bool {
    state.revision == 0
        && state.next_invalidation_id == 1
        && state.reviews.is_empty()
        && state.invalidations.is_empty()
}

/// Encode one content reference: `0` plus an inline descriptor for a unique
/// descriptor, or `table_index + 1` for a repeated one.
fn put_content(out: &mut Vec<u8>, tables: &Tables, descriptor: (u64, Hash64)) {
    match tables.content_id.get(&descriptor) {
        Some(index) => put_u(out, *index as u64 + 1),
        None => {
            put_u(out, 0);
            put_u(out, descriptor.0);
            put_h(out, descriptor.1);
        }
    }
}

/// Charge the logical expansion of one state against a reader budget.
///
/// The reader charges every materialized occurrence: each path or string it
/// rebuilds from a table, each element of an integer vector, and the owner
/// prefix of each reconstructed file path. This walk charges the same
/// occurrences from the typed state, in the same units, before the writer
/// allocates a single table. A state whose logical expansion exceeds a
/// reader budget is therefore refused before any allocation, and the
/// previous file is left untouched.
///
/// The walk never charges more than the reader does. It charges no scalar
/// for the structural integers of the wire format, so it is a lower bound on
/// the reader's value count; [`encode`] still decodes its own output, which
/// makes the check exact.
fn measure_expansion(state: &ReviewState) -> Result<(), LockError> {
    let mut budget = Budget::default();
    for (document, record) in &state.reviews {
        budget.spend_string_bytes(document.as_str().len() as u64)?;
        if let Some(commit) = &record.git.base_commit {
            budget.spend_string_bytes(commit.len() as u64)?;
        }
        budget.spend_string_bytes(record.reviewer.as_str().len() as u64)?;
        budget.spend_string_bytes(record.note.as_str().len() as u64)?;
        let acknowledged = record.acknowledged_invalidations.len() as u64;
        budget.spend_values(acknowledged)?;
        budget.spend_string_bytes(acknowledged.saturating_mul(8))?;
        let owner = document.directory();
        let owner_bytes = owner.as_str().len() as u64;
        for file in record.manifest.files() {
            // The reader stores the path relative to its owner and rebuilds
            // the whole path, so both halves are materialized.
            let full = file.path.as_str().len() as u64;
            let relative = if owner.is_root() {
                full
            } else {
                full.saturating_sub(owner_bytes + 1)
            };
            budget.spend_string_bytes(relative + owner_bytes)?;
        }
        for import in record.manifest.imports() {
            budget.spend_string_bytes(import.document.as_str().len() as u64)?;
            budget.spend_string_bytes(import.export_id.as_str().len() as u64)?;
        }
    }
    for invalidation in &state.invalidations {
        budget.spend_string_bytes(invalidation.reason.as_str().len() as u64)?;
        match &invalidation.scope {
            InvalidationScope::All => {}
            InvalidationScope::Document(document) => {
                budget.spend_string_bytes(document.as_str().len() as u64)?;
            }
            InvalidationScope::Subtree(directory) => {
                budget.spend_string_bytes(directory.as_str().len() as u64)?;
            }
        }
        for documents in [&invalidation.targets, &invalidation.pending] {
            let count = documents.len() as u64;
            budget.spend_values(count)?;
            budget.spend_string_bytes(count.saturating_mul(8))?;
            for document in documents {
                budget.spend_string_bytes(document.as_str().len() as u64)?;
            }
        }
    }
    Ok(())
}

/// The canonical normalized payload of one state.
pub fn encode_payload(state: &ReviewState) -> Result<Vec<u8>, LockError> {
    if is_initial(state) {
        return Ok(Vec::new());
    }
    let tables = build_tables(state)?;
    let mut out = Vec::new();
    put_u(&mut out, state.revision);
    put_u(&mut out, state.next_invalidation_id);
    put_z(&mut out, tables.base_time);

    // 1. Strings, front-coded against the preceding entry.
    put_u(&mut out, tables.strings.len() as u64);
    let mut previous: &[u8] = b"";
    for value in &tables.strings {
        let raw = value.as_bytes();
        let shared = raw.iter().zip(previous).take_while(|(a, b)| a == b).count();
        put_u(&mut out, shared as u64);
        put_u(&mut out, (raw.len() - shared) as u64);
        out.extend_from_slice(&raw[shared..]);
        previous = raw;
    }

    // 2. Paths, as a trie whose parents always precede their children.
    put_u(&mut out, (tables.paths.len() - 1) as u64);
    for path in &tables.paths[1..] {
        let (parent, leaf) = match path.rfind('/') {
            Some(index) => (&path[..index], &path[index + 1..]),
            None => ("", path.as_str()),
        };
        put_u(&mut out, tables.path_id[parent] as u64);
        put_u(&mut out, tables.string_id[leaf] as u64);
    }

    // 3. Repeated content descriptors.
    put_u(&mut out, tables.contents.len() as u64);
    for (bytes, hash) in &tables.contents {
        put_u(&mut out, *bytes);
        put_h(&mut out, *hash);
    }

    // 4. Guidance digests.
    put_u(&mut out, tables.guidance.len() as u64);
    for digest in &tables.guidance {
        put_h(&mut out, *digest);
    }

    // 5. Git contexts.
    put_u(&mut out, tables.gits.len() as u64);
    for wire in &tables.gits {
        out.extend_from_slice(wire);
    }

    // 6. Integer vectors, delta encoded.
    put_u(&mut out, tables.vectors.len() as u64);
    for vector in &tables.vectors {
        put_u(&mut out, vector.len() as u64);
        let mut previous = 0u64;
        for value in vector {
            put_u(&mut out, value - previous);
            previous = *value;
        }
    }

    // 7. Reviews, ordered by the numeric path index of their document.
    let mut ordered: Vec<(&DocumentId, &ReviewRecord)> = state.reviews.iter().collect();
    ordered.sort_by_key(|(document, _)| tables.path_id[document.as_str()]);
    put_u(&mut out, ordered.len() as u64);
    let mut previous_document = 0u64;
    for (document, record) in ordered {
        let manifest = &record.manifest;
        let index = tables.path_id[document.as_str()] as u64;
        put_u(&mut out, index - previous_document);
        previous_document = index;
        put_u(&mut out, record.revision);
        put_h(&mut out, manifest.policy_hash);
        put_content(
            &mut out,
            &tables,
            (manifest.document_bytes, manifest.document_hash),
        );
        put_u(&mut out, tables.guidance_id[&record.guidance.0] as u64);
        let reviewed = timestamp_seconds(&record.reviewed_at)?;
        put_u(&mut out, (reviewed - tables.base_time) as u64);
        put_u(&mut out, tables.string_id[record.reviewer.as_str()] as u64);
        out.push(match record.result {
            ReviewResult::Updated => 0,
            ReviewResult::NoUpdate => 1,
        });
        put_u(&mut out, tables.string_id[record.note.as_str()] as u64);
        put_u(&mut out, tables.git_id[&git_wire(&record.git)?] as u64);
        put_h(&mut out, record.token_digest);
        put_u(
            &mut out,
            tables.vector_id[&record.acknowledged_invalidations] as u64,
        );

        let mut files: Vec<(usize, &FileInput)> = Vec::with_capacity(manifest.files().len());
        for file in manifest.files() {
            let relative = relative_to_owner(document, &file.path)?;
            files.push((tables.string_id[&relative], file));
        }
        files.sort_by_key(|(index, _)| *index);
        put_u(&mut out, files.len() as u64);
        let mut previous_file = 0u64;
        for (index, file) in files {
            put_u(&mut out, index as u64 - previous_file);
            previous_file = index as u64;
            put_content(&mut out, &tables, (file.bytes, file.hash));
        }

        let mut imports: Vec<(usize, usize, &ImportInput)> = manifest
            .imports()
            .iter()
            .map(|import| {
                (
                    tables.path_id[import.document.as_str()],
                    tables.string_id[import.export_id.as_str()],
                    import,
                )
            })
            .collect();
        imports.sort_by_key(|(provider, export, _)| (*provider, *export));
        put_u(&mut out, imports.len() as u64);
        for (provider, export, import) in imports {
            put_u(&mut out, provider as u64);
            put_u(&mut out, export as u64);
            put_content(&mut out, &tables, (import.bytes, import.hash));
        }
    }

    // 8. Open invalidations, ordered by identifier.
    put_u(&mut out, state.invalidations.len() as u64);
    let mut previous_id = 0u64;
    for invalidation in &state.invalidations {
        put_u(&mut out, invalidation.id - previous_id);
        previous_id = invalidation.id;
        match &invalidation.scope {
            InvalidationScope::All => out.push(0),
            InvalidationScope::Document(document) => {
                out.push(1);
                put_u(&mut out, tables.path_id[document.as_str()] as u64);
            }
            InvalidationScope::Subtree(dir) => {
                out.push(2);
                put_u(&mut out, tables.path_id[dir.as_str()] as u64);
            }
        }
        let created = timestamp_seconds(&invalidation.created_at)?;
        put_u(&mut out, (created - tables.base_time) as u64);
        put_u(
            &mut out,
            tables.string_id[invalidation.reason.as_str()] as u64,
        );
        for group in [&invalidation.targets, &invalidation.pending] {
            let mut ids: Vec<u64> = group
                .iter()
                .map(|document| tables.path_id[document.as_str()] as u64)
                .collect();
            ids.sort_unstable();
            put_u(&mut out, tables.vector_id[&ids] as u64);
        }
    }
    Ok(out)
}

// --------------------------------------------------------------- compression

/// The exact compression profile of codec 1.
///
/// The search parameters select the measured level 19 behavior. The bundled
/// library leaves `compressionLevel` at 3 after expanding that preset, so
/// the canonical profile fixes that value too. A future compressor change
/// that changes canonical bytes needs a new codec identifier or format
/// version; it cannot silently redefine codec 1.
fn compress(payload: &[u8]) -> Result<Vec<u8>, LockError> {
    use zstd_safe::{CCtx, CParameter};

    let mut context = CCtx::create();
    let set = |context: &mut CCtx<'_>, parameter: CParameter| -> Result<(), LockError> {
        context
            .set_parameter(parameter)
            .map_err(|code| LockError::Corrupt(format!("cannot configure the compressor: {code}")))
            .map(|_| ())
    };
    set(&mut context, CParameter::CompressionLevel(3))?;
    set(&mut context, CParameter::WindowLog(20))?;
    set(&mut context, CParameter::ChainLog(24))?;
    set(&mut context, CParameter::HashLog(22))?;
    set(&mut context, CParameter::SearchLog(7))?;
    set(&mut context, CParameter::MinMatch(3))?;
    set(&mut context, CParameter::TargetLength(256))?;
    set(
        &mut context,
        CParameter::Strategy(zstd_safe::Strategy::ZSTD_btultra2),
    )?;
    set(&mut context, CParameter::NbWorkers(0))?;
    set(&mut context, CParameter::EnableLongDistanceMatching(false))?;
    set(&mut context, CParameter::ContentSizeFlag(true))?;
    set(&mut context, CParameter::ChecksumFlag(false))?;
    set(&mut context, CParameter::DictIdFlag(false))?;
    // The ordinary Zstandard frame format is the library default; the
    // magicless format stays behind an experimental feature that is not
    // enabled here, so codec 1 cannot accidentally use it.
    // The exact source size must reach the frame header, so the decoder can
    // compare the declared content size with the outer decoded length.
    context
        .set_pledged_src_size(Some(payload.len() as u64))
        .map_err(|code| LockError::Corrupt(format!("cannot pledge the source size: {code}")))?;
    let mut out = Vec::with_capacity(zstd_safe::compress_bound(payload.len()));
    context
        .compress2(&mut out, payload)
        .map_err(|code| LockError::Limit(format!("compression failed with code {code}")))?;
    Ok(out)
}

/// The little-endian magic number of an ordinary Zstandard frame.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// Validate the frame against the frozen profile, before decompression.
///
/// Codec 1 is exactly one ordinary Zstandard frame with a content size, no
/// content checksum, and no dictionary identifier. A skippable frame, a
/// dictionary frame, a checksummed frame, and any trailing or concatenated
/// frame are all rejected here, so the decompressor never sees them.
fn validate_frame_profile(body: &[u8]) -> Result<(), LockError> {
    if body.len() < 5 {
        return corrupt("the compressed body is shorter than one Zstandard frame header");
    }
    if body[..4] != ZSTD_MAGIC {
        return corrupt(
            "the compressed body is not an ordinary Zstandard frame; skippable and magicless frames are not part of codec 1",
        );
    }
    // Frame_Header_Descriptor, as RFC 8878 defines it. Bit 2 is the content
    // checksum flag, bits 1 and 0 are the dictionary identifier flag, and
    // bits 4 and 3 must be zero.
    let descriptor = body[4];
    if descriptor & 0b0000_0100 != 0 {
        return corrupt(
            "the compressed body carries a Zstandard content checksum; codec 1 relies on the outer XXH3-128 trailer only",
        );
    }
    if descriptor & 0b0000_0011 != 0 {
        return corrupt("the compressed body names a dictionary identifier");
    }
    if descriptor & 0b0001_1000 != 0 {
        return corrupt("the compressed body sets a reserved frame header bit");
    }
    // Exactly one frame: the first frame must consume every body byte, so a
    // valid frame followed by another frame, an empty frame, or padding is
    // rejected.
    let first = zstd_safe::find_frame_compressed_size(body).map_err(|_| {
        LockError::Corrupt("the compressed body is not a readable Zstandard frame".into())
    })?;
    if first != body.len() {
        return corrupt(format!(
            "the compressed body holds {} bytes after its first frame; codec 1 stores exactly one frame",
            body.len() - first
        ));
    }
    Ok(())
}

/// Decompress one ordinary Zstandard frame under the decoding constraints.
fn decompress(body: &[u8], expected: usize) -> Result<Vec<u8>, LockError> {
    use zstd_safe::DCtx;

    validate_frame_profile(body)?;
    let declared = zstd_safe::get_frame_content_size(body).map_err(|_| {
        LockError::Corrupt("the compressed body is not a readable Zstandard frame".into())
    })?;
    match declared {
        None => {
            return corrupt("the compressed body declares no content size");
        }
        Some(size) if size != expected as u64 => {
            return corrupt(format!(
                "the compressed body declares {size} bytes but the frame header says {expected}"
            ));
        }
        Some(_) => {}
    }
    let mut context = DCtx::create();
    context
        .set_parameter(zstd_safe::DParameter::WindowLogMax(MAX_WINDOW_LOG))
        .map_err(|code| LockError::Corrupt(format!("cannot bound the window: {code}")))?;
    let mut out = Vec::with_capacity(expected);
    let written = context.decompress(&mut out, body).map_err(|code| {
        LockError::Corrupt(format!(
            "the compressed body does not decode under the bounded profile (code {code})"
        ))
    })?;
    if written != expected || out.len() != expected {
        return corrupt("the decompressed payload does not have the declared length");
    }
    Ok(out)
}

/// Encode one complete `memoria.lock` file, and prove the reader accepts it.
///
/// The writer and the reader must enforce identical limits. Rather than
/// duplicating the expansion accounting on both sides, where the two copies
/// could drift, the writer decodes its own output before returning it. A
/// state that would expand past a reader budget therefore fails at encode
/// time, before any allocation of the file and before replacement.
///
/// The read-back is preceded by a logical preflight, so an oversized state
/// is refused before the writer builds a table or allocates the file.
///
/// The verification is exact by construction: it runs the production
/// decoder over the production bytes.
pub fn encode(state: &ReviewState) -> Result<Vec<u8>, LockError> {
    // Table construction materializes every expanded string, so the budget
    // is checked from the typed state first, before any allocation.
    measure_expansion(state).map_err(|err| match err {
        LockError::Limit(message) => LockError::Limit(format!(
            "refusing to write state that this release cannot read back: {message}"
        )),
        other => other,
    })?;
    let bytes = encode_unverified(state)?;
    match decode(&bytes) {
        Ok(_) => Ok(bytes),
        Err(LockError::Limit(message)) => Err(LockError::Limit(format!(
            "refusing to write state that this release cannot read back: {message}"
        ))),
        Err(other) => Err(LockError::Corrupt(format!(
            "refusing to write state that this release cannot read back: {other}"
        ))),
    }
}

/// Encode without the logical preflight or the read-back proof.
///
/// This exists for fixtures and diagnostics that deliberately construct
/// bytes the reader must reject. Product writes use [`encode`].
pub fn encode_unverified(state: &ReviewState) -> Result<Vec<u8>, LockError> {
    let payload = encode_payload(state)?;
    if payload.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(LockError::Limit(format!(
            "the encoded payload is {} bytes, above the limit of {MAX_PAYLOAD_BYTES}",
            payload.len()
        )));
    }
    let compressed = if payload.is_empty() {
        Vec::new()
    } else {
        compress(&payload)?
    };
    let (codec, body): (u8, &[u8]) = if !compressed.is_empty() && compressed.len() < payload.len() {
        (CODEC_ZSTD, &compressed)
    } else {
        (CODEC_RAW, &payload)
    };
    let mut wire = Vec::with_capacity(body.len() + 32);
    wire.extend_from_slice(&MAGIC);
    wire.push(FORMAT_VERSION);
    wire.push(codec);
    put_u(&mut wire, payload.len() as u64);
    wire.extend_from_slice(body);
    let checksum = xxh3_128(&wire);
    wire.extend_from_slice(&checksum);
    if wire.len() as u64 > MAX_FILE_BYTES {
        return Err(LockError::Limit(format!(
            "the encoded file is {} bytes, above the limit of {MAX_FILE_BYTES}",
            wire.len()
        )));
    }
    Ok(wire)
}

/// Decode one complete `memoria.lock` file with every strict check.
pub fn decode(bytes: &[u8]) -> Result<DecodedLock, LockError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(LockError::Limit(format!(
            "the state file is {} bytes, above the limit of {MAX_FILE_BYTES}",
            bytes.len()
        )));
    }
    if bytes.is_empty() {
        return corrupt("the state file is empty");
    }
    // 1. Framing.
    if bytes.len() < 4 + 1 + 1 + 1 + 16 {
        return corrupt("the state file is shorter than one complete frame");
    }
    if bytes[..4] != MAGIC {
        return corrupt("the state file does not begin with the memoria.lock magic bytes");
    }
    let format_version = bytes[4];
    if format_version != FORMAT_VERSION {
        return Err(LockError::UnsupportedSchema(format!(
            "unsupported memoria.lock format version {format_version}; this release supports version {FORMAT_VERSION} only"
        )));
    }
    let codec = bytes[5];
    if codec != CODEC_RAW && codec != CODEC_ZSTD {
        return Err(LockError::UnsupportedCodec(format!(
            "unsupported memoria.lock codec {codec}; this release supports codecs 0 and 1"
        )));
    }
    // 2. Checksum before decompression.
    let split = bytes.len() - 16;
    let expected = &bytes[split..];
    let actual = xxh3_128(&bytes[..split]);
    if expected != actual {
        return corrupt(format!(
            "the state file checksum {} does not match its contents ({})",
            encode_hex(expected),
            encode_hex(&actual)
        ));
    }
    // 3. Bounded body with an exact decoded length.
    let mut header = Reader::new(&bytes[6..split]);
    let declared = header.u()?;
    if declared > MAX_PAYLOAD_BYTES {
        return Err(LockError::Limit(format!(
            "the state file declares a {declared}-byte payload, above the limit of {MAX_PAYLOAD_BYTES}"
        )));
    }
    let body = &bytes[6 + header.position..split];
    let payload = match codec {
        CODEC_RAW => {
            if body.len() as u64 != declared {
                return corrupt(format!(
                    "the raw body has {} bytes but the frame declares {declared}",
                    body.len()
                ));
            }
            body.to_vec()
        }
        _ => decompress(body, declared as usize)?,
    };
    // 4 and 5. Canonical tables, references, and domain invariants.
    let (state, guidance) = decode_payload(&payload)?;
    Ok(DecodedLock {
        file_bytes: bytes.len() as u64,
        payload_bytes: payload.len() as u64,
        format_version,
        codec,
        checksum: encode_hex(expected),
        state,
        guidance,
    })
}

// ------------------------------------------------------------ payload decode

struct DecodedTables {
    strings: Vec<String>,
    paths: Vec<String>,
    contents: Vec<(u64, Hash64)>,
    guidance: Vec<Hash64>,
    gits: Vec<GitContext>,
    vectors: Vec<Vec<u64>>,
    base_time: i64,
    /// How often each table entry was referenced, so unused rows are caught.
    string_uses: Vec<u64>,
    path_uses: Vec<u64>,
    content_uses: Vec<u64>,
    guidance_uses: Vec<u64>,
    git_uses: Vec<u64>,
    vector_uses: Vec<u64>,
    /// How often each unique descriptor occurred inline.
    inline_counts: BTreeMap<(u64, Hash64), u64>,
}

fn index<T>(items: &[T], value: u64, what: &str) -> Result<usize, LockError> {
    let position = usize::try_from(value)
        .map_err(|_| LockError::Corrupt(format!("{what} index {value} is out of range")))?;
    if position >= items.len() {
        return Err(LockError::Corrupt(format!(
            "{what} index {value} is beyond the {} table entries",
            items.len()
        )));
    }
    Ok(position)
}

fn decode_payload(
    payload: &[u8],
) -> Result<(ReviewState, BTreeMap<DocumentId, GuidanceDigest>), LockError> {
    if payload.is_empty() {
        return Ok((ReviewState::empty(), BTreeMap::new()));
    }
    let mut r = Reader::new(payload);
    let revision = r.u()?;
    let next_invalidation_id = r.u()?;
    let base_time = r.z()?;

    // 1. Strings.
    let string_count = r.u()?;
    let mut strings: Vec<String> = Vec::new();
    let mut previous: Vec<u8> = Vec::new();
    for position in 0..string_count {
        let shared = r.u()? as usize;
        let suffix_len = r.u()? as usize;
        if shared > previous.len() {
            return corrupt(format!(
                "string {position} shares {shared} bytes with a {} byte predecessor",
                previous.len()
            ));
        }
        if position == 0 && shared != 0 {
            return corrupt("the first string cannot share a prefix");
        }
        r.spend_string_bytes((shared + suffix_len) as u64)?;
        let mut raw = previous[..shared].to_vec();
        raw.extend_from_slice(r.take(suffix_len)?);
        let text = String::from_utf8(raw.clone())
            .map_err(|_| LockError::Corrupt(format!("string {position} is not valid UTF-8")))?;
        if let Some(last) = strings.last()
            && last.as_bytes() >= text.as_bytes()
        {
            return corrupt(format!(
                "strings must be unique and sorted by their bytes; {text:?} follows {last:?}"
            ));
        }
        // A shortened shared prefix is noncanonical: the encoder always uses
        // the longest shared prefix.
        if let Some(last) = strings.last() {
            let longest = last
                .as_bytes()
                .iter()
                .zip(&raw)
                .take_while(|(a, b)| a == b)
                .count();
            if longest != shared {
                return corrupt(format!(
                    "string {position} declares a {shared} byte prefix but shares {longest}"
                ));
            }
        }
        previous = raw;
        strings.push(text);
    }

    // 2. Paths.
    let path_count = r.u()?;
    let mut paths: Vec<String> = vec![String::new()];
    let mut string_uses = vec![0u64; strings.len()];
    let mut path_uses = vec![0u64; 1];
    for position in 0..path_count {
        let parent = index(&paths, r.u()?, "path")?;
        let leaf_index = index(&strings, r.u()?, "string")?;
        let leaf = &strings[leaf_index];
        if leaf.is_empty() || leaf.contains('/') || leaf == "." || leaf == ".." {
            return corrupt(format!("path component {leaf:?} is not a valid component"));
        }
        string_uses[leaf_index] += 1;
        path_uses[parent] += 1;
        let joined = if paths[parent].is_empty() {
            leaf.clone()
        } else {
            format!("{}/{leaf}", paths[parent])
        };
        r.spend_string_bytes(joined.len() as u64)?;
        if let Some(last) = paths.last()
            && position > 0
            && path_key(last) >= path_key(&joined)
        {
            return corrupt(format!(
                "paths must be unique and sorted by depth then bytes; {joined:?} follows {last:?}"
            ));
        }
        paths.push(joined);
        path_uses.push(0);
    }

    // 3. Repeated content descriptors.
    let content_count = r.u()?;
    let mut contents: Vec<(u64, Hash64)> = Vec::new();
    for _ in 0..content_count {
        let bytes = r.u()?;
        let hash = r.h()?;
        if let Some(last) = contents.last()
            && *last >= (bytes, hash)
        {
            return corrupt("repeated content descriptors must be unique and sorted");
        }
        contents.push((bytes, hash));
    }

    // 4. Guidance digests.
    let guidance_count = r.u()?;
    let mut guidance: Vec<Hash64> = Vec::new();
    for _ in 0..guidance_count {
        let digest = r.h()?;
        if let Some(last) = guidance.last()
            && *last >= digest
        {
            return corrupt("guidance digests must be unique and sorted");
        }
        guidance.push(digest);
    }

    // 5. Git contexts.
    let git_count = r.u()?;
    let mut gits: Vec<GitContext> = Vec::new();
    let mut git_wires: Vec<Vec<u8>> = Vec::new();
    for _ in 0..git_count {
        let flag = r.byte()?;
        if flag & !0b111 != 0 || (flag & 0b110) == 0b110 {
            return corrupt(format!("git context flag {flag} is not a defined value"));
        }
        let object_len = match flag & 0b110 {
            0b010 => 20,
            0b100 => 32,
            _ => 0,
        };
        let object = r.take(object_len)?.to_vec();
        let mut wire = vec![flag];
        wire.extend_from_slice(&object);
        if let Some(last) = git_wires.last()
            && last.as_slice() >= wire.as_slice()
        {
            return corrupt("git contexts must be unique and sorted by their encoded bytes");
        }
        git_wires.push(wire);
        gits.push(GitContext {
            base_commit: (object_len > 0).then(|| encode_hex(&object)),
            worktree_dirty: flag & 1 == 1,
        });
    }

    // 6. Integer vectors.
    let vector_count = r.u()?;
    let mut vectors: Vec<Vec<u64>> = Vec::new();
    for _ in 0..vector_count {
        let length = r.u()?;
        let mut vector: Vec<u64> = Vec::new();
        let mut previous_value = 0u64;
        for position in 0..length {
            let delta = r.u()?;
            if position > 0 && delta == 0 {
                return corrupt("an integer vector repeats a value; vectors are sorted sets");
            }
            previous_value = previous_value
                .checked_add(delta)
                .ok_or_else(|| LockError::Corrupt("an integer vector overflows".into()))?;
            vector.push(previous_value);
        }
        if let Some(last) = vectors.last()
            && last >= &vector
        {
            return corrupt("integer vectors must be unique and sorted");
        }
        vectors.push(vector);
    }

    let mut tables = DecodedTables {
        strings,
        paths,
        contents,
        guidance,
        gits,
        vectors,
        base_time,
        string_uses,
        path_uses,
        content_uses: vec![0; content_count as usize],
        guidance_uses: vec![0; guidance_count as usize],
        git_uses: vec![0; git_count as usize],
        vector_uses: vec![0; vector_count as usize],
        inline_counts: BTreeMap::new(),
    };

    let (reviews, guidance_map) = decode_reviews(&mut r, &mut tables)?;
    let invalidations = decode_invalidations(&mut r, &mut tables)?;
    r.finish()?;

    // Canonical tables carry no unused rows, and every repeated descriptor
    // uses the table while every unique one stays inline.
    check_uses(&tables)?;

    let state = ReviewState {
        revision,
        next_invalidation_id,
        reviews,
        invalidations,
    };
    if is_initial(&state) {
        return corrupt("the initial empty state must use the empty payload, not the full schema");
    }
    if base_time != minimal_base_time(&state)? {
        return corrupt("base_time is not the earliest stored review or invalidation time");
    }
    state
        .validate()
        .map_err(|err| LockError::Corrupt(err.to_string()))?;
    for record in state.reviews.values() {
        for id in &record.acknowledged_invalidations {
            if *id >= state.next_invalidation_id {
                return corrupt(format!(
                    "acknowledged invalidation {id} is beyond the counter"
                ));
            }
        }
    }
    Ok((state, guidance_map))
}

fn minimal_base_time(state: &ReviewState) -> Result<i64, LockError> {
    let mut times: Vec<i64> = Vec::new();
    for record in state.reviews.values() {
        times.push(timestamp_seconds(&record.reviewed_at)?);
    }
    for invalidation in &state.invalidations {
        times.push(timestamp_seconds(&invalidation.created_at)?);
    }
    Ok(times.into_iter().min().unwrap_or(0))
}

fn check_uses(tables: &DecodedTables) -> Result<(), LockError> {
    let unused = |uses: &[u64], what: &str| -> Result<(), LockError> {
        if let Some(position) = uses.iter().position(|count| *count == 0) {
            return Err(LockError::Corrupt(format!(
                "{what} table entry {position} is never referenced"
            )));
        }
        Ok(())
    };
    unused(&tables.string_uses, "string")?;
    // Path index 0 is the implicit root. It may exist without a reference
    // only when a stored row needs it as an ancestor.
    unused(&tables.path_uses[1..], "path")?;
    unused(&tables.content_uses, "repeated content")?;
    unused(&tables.guidance_uses, "guidance")?;
    unused(&tables.git_uses, "git context")?;
    unused(&tables.vector_uses, "integer vector")?;
    for (descriptor, count) in &tables.inline_counts {
        if *count > 1 {
            return Err(LockError::Corrupt(format!(
                "content descriptor ({}, {}) occurs {count} times inline; repeated descriptors must use the table",
                descriptor.0, descriptor.1
            )));
        }
    }
    Ok(())
}

fn take_content(
    r: &mut Reader<'_>,
    tables: &mut DecodedTables,
) -> Result<(u64, Hash64), LockError> {
    let tag = r.u()?;
    if tag == 0 {
        let bytes = r.u()?;
        let hash = r.h()?;
        let descriptor = (bytes, hash);
        if tables.contents.contains(&descriptor) {
            return corrupt(
                "an inline content descriptor duplicates a repeated-content table entry",
            );
        }
        *tables.inline_counts.entry(descriptor).or_insert(0) += 1;
        return Ok(descriptor);
    }
    // A table reference expands into the same two scalars an inline
    // descriptor would spend, so sharing cannot bypass the value budget.
    r.spend_values(2)?;
    let position = index(&tables.contents, tag - 1, "repeated content")?;
    tables.content_uses[position] += 1;
    Ok(tables.contents[position])
}

/// Materialize one table path by identifier, charging its expanded bytes.
///
/// Every materialized occurrence is charged, not only the table row, so many
/// references to one long path cannot expand past the limit. Every path that
/// leaves a table goes through this function.
fn charged_path(
    r: &mut Reader<'_>,
    tables: &mut DecodedTables,
    id: u64,
) -> Result<String, LockError> {
    let position = index(&tables.paths, id, "path")?;
    tables.path_uses[position] += 1;
    r.spend_string_bytes(tables.paths[position].len() as u64)?;
    Ok(tables.paths[position].clone())
}

/// Materialize one table string by identifier, charging its expanded bytes.
fn charged_string(
    r: &mut Reader<'_>,
    tables: &mut DecodedTables,
    id: u64,
) -> Result<String, LockError> {
    let position = index(&tables.strings, id, "string")?;
    tables.string_uses[position] += 1;
    r.spend_string_bytes(tables.strings[position].len() as u64)?;
    Ok(tables.strings[position].clone())
}

/// Materialize one Git context, charging the bytes of its commit text.
fn charged_git(r: &mut Reader<'_>, tables: &mut DecodedTables) -> Result<GitContext, LockError> {
    let position = index(&tables.gits, r.u()?, "git context")?;
    tables.git_uses[position] += 1;
    // Charged before the clone, like every other materialization.
    if let Some(commit) = &tables.gits[position].base_commit {
        let bytes = commit.len() as u64;
        r.spend_string_bytes(bytes)?;
    }
    Ok(tables.gits[position].clone())
}

fn take_path(r: &mut Reader<'_>, tables: &mut DecodedTables) -> Result<String, LockError> {
    let id = r.u()?;
    charged_path(r, tables, id)
}

fn take_string(r: &mut Reader<'_>, tables: &mut DecodedTables) -> Result<String, LockError> {
    let id = r.u()?;
    charged_string(r, tables, id)
}

fn take_vector(r: &mut Reader<'_>, tables: &mut DecodedTables) -> Result<Vec<u64>, LockError> {
    let position = index(&tables.vectors, r.u()?, "integer vector")?;
    tables.vector_uses[position] += 1;
    let length = tables.vectors[position].len() as u64;
    // Each expanded element is a scalar value as well as eight bytes.
    r.spend_values(length)?;
    r.spend_string_bytes(length * 8)?;
    Ok(tables.vectors[position].clone())
}

fn timestamp_from(tables: &DecodedTables, delta: u64) -> Result<Timestamp, LockError> {
    let seconds = tables
        .base_time
        .checked_add(i64::try_from(delta).map_err(|_| {
            LockError::Corrupt("a stored time delta does not fit in 64 bits".into())
        })?)
        .ok_or_else(|| LockError::Corrupt("a stored time overflows".into()))?;
    let text = format_timestamp(seconds).ok_or_else(|| {
        LockError::Corrupt("a stored time is outside the years 0001 through 9999".into())
    })?;
    Ok(Timestamp(text))
}

#[allow(clippy::type_complexity)]
fn decode_reviews(
    r: &mut Reader<'_>,
    tables: &mut DecodedTables,
) -> Result<
    (
        BTreeMap<DocumentId, ReviewRecord>,
        BTreeMap<DocumentId, GuidanceDigest>,
    ),
    LockError,
> {
    let count = r.u()?;
    let mut reviews = BTreeMap::new();
    let mut digests = BTreeMap::new();
    let mut previous_document = 0u64;
    for position in 0..count {
        let delta = r.u()?;
        if position > 0 && delta == 0 {
            return corrupt("review rows must name strictly increasing documents");
        }
        previous_document = previous_document
            .checked_add(delta)
            .ok_or_else(|| LockError::Corrupt("a document index overflows".into()))?;
        let raw_document = charged_path(r, tables, previous_document)?;
        let document = DocumentId::parse(&raw_document).map_err(|err| {
            LockError::Corrupt(format!("review row names {raw_document:?}: {err}"))
        })?;
        let revision = r.u()?;
        let policy_hash = r.h()?;
        let (document_bytes, document_hash) = take_content(r, tables)?;
        let guidance_index = index(&tables.guidance, r.u()?, "guidance")?;
        tables.guidance_uses[guidance_index] += 1;
        let guidance = GuidanceDigest(tables.guidance[guidance_index]);
        let reviewed_at = timestamp_from(tables, r.u()?)?;
        let reviewer_text = take_string(r, tables)?;
        let reviewer = ReviewerName::parse(&reviewer_text).map_err(|err| {
            LockError::Corrupt(format!(
                "review row for {document} has an invalid reviewer: {err}"
            ))
        })?;
        let result = match r.byte()? {
            0 => ReviewResult::Updated,
            1 => ReviewResult::NoUpdate,
            other => return corrupt(format!("review result tag {other} is not 0 or 1")),
        };
        let note_text = take_string(r, tables)?;
        let note = ReviewNote::parse(&note_text).map_err(|err| {
            LockError::Corrupt(format!(
                "review row for {document} has an invalid note: {err}"
            ))
        })?;
        let git = charged_git(r, tables)?;
        let token_digest = r.h()?;
        let acknowledged = take_vector(r, tables)?;

        let owner = document.directory();
        let file_count = r.u()?;
        let mut files: Vec<FileInput> = Vec::new();
        let mut previous_file = 0u64;
        for position in 0..file_count {
            let delta = r.u()?;
            if position > 0 && delta == 0 {
                return corrupt("file rows must name strictly increasing relative paths");
            }
            previous_file = previous_file
                .checked_add(delta)
                .ok_or_else(|| LockError::Corrupt("a file index overflows".into()))?;
            let relative = charged_string(r, tables, previous_file)?;
            // The reconstructed path also materializes its owner prefix.
            r.spend_string_bytes(owner.as_str().len() as u64)?;
            let path = owner.join(&relative).map_err(|err| {
                LockError::Corrupt(format!(
                    "review row for {document} stores relative path {relative:?}: {err}"
                ))
            })?;
            if !path.is_within(&owner) {
                return corrupt(format!(
                    "review row for {document} stores {path}, which escapes its boundary"
                ));
            }
            let (bytes, hash) = take_content(r, tables)?;
            files.push(FileInput { path, bytes, hash });
        }

        let import_count = r.u()?;
        let mut imports: Vec<ImportInput> = Vec::new();
        let mut previous_import: Option<(u64, u64)> = None;
        for _ in 0..import_count {
            let provider_index = r.u()?;
            let export_index = r.u()?;
            if let Some(previous) = previous_import
                && previous >= (provider_index, export_index)
            {
                return corrupt("import rows must sort by provider index then export index");
            }
            previous_import = Some((provider_index, export_index));
            let raw_provider = charged_path(r, tables, provider_index)?;
            let provider = DocumentId::parse(&raw_provider).map_err(|err| {
                LockError::Corrupt(format!("import row names {raw_provider:?}: {err}"))
            })?;
            let export_text = charged_string(r, tables, export_index)?;
            let export_id = ExportId::parse(&export_text).map_err(|err| {
                LockError::Corrupt(format!("import row has an invalid export id: {err}"))
            })?;
            let (bytes, hash) = take_content(r, tables)?;
            imports.push(ImportInput {
                document: provider,
                export_id,
                bytes,
                hash,
            });
        }

        let sorted_files = {
            let mut copy = files.clone();
            copy.sort_by(|a, b| a.path.cmp(&b.path));
            copy
        };
        let manifest = InputManifest::new(
            document.clone(),
            policy_hash,
            document_bytes,
            document_hash,
            files,
            imports,
        )
        .map_err(|err| LockError::Corrupt(err.to_string()))?;
        if manifest.files() != sorted_files.as_slice() {
            return corrupt(format!(
                "review row for {document} lists duplicate or unorderable file paths"
            ));
        }
        // The fingerprint is derived, never a second stored checksum.
        let input_fingerprint = Hash64(xxh3_64(&canonical::encode_inputs(&manifest)));
        if reviews
            .insert(
                document.clone(),
                ReviewRecord {
                    revision,
                    manifest,
                    input_fingerprint,
                    token_digest,
                    guidance,
                    reviewed_at,
                    reviewer,
                    result,
                    note,
                    git,
                    acknowledged_invalidations: acknowledged,
                },
            )
            .is_some()
        {
            return corrupt(format!("review rows name {document} twice"));
        }
        digests.insert(document, guidance);
    }
    Ok((reviews, digests))
}

fn decode_invalidations(
    r: &mut Reader<'_>,
    tables: &mut DecodedTables,
) -> Result<Vec<Invalidation>, LockError> {
    let count = r.u()?;
    let mut out: Vec<Invalidation> = Vec::new();
    let mut previous_id = 0u64;
    for position in 0..count {
        let delta = r.u()?;
        if position > 0 && delta == 0 {
            return corrupt("invalidation rows must have strictly increasing identifiers");
        }
        previous_id = previous_id
            .checked_add(delta)
            .ok_or_else(|| LockError::Corrupt("an invalidation identifier overflows".into()))?;
        let scope = match r.byte()? {
            0 => InvalidationScope::All,
            1 => {
                let raw = take_path(r, tables)?;
                InvalidationScope::Document(DocumentId::parse(&raw).map_err(|err| {
                    LockError::Corrupt(format!("invalidation scope names {raw:?}: {err}"))
                })?)
            }
            2 => {
                let raw = take_path(r, tables)?;
                InvalidationScope::Subtree(DirPath::parse(&raw).map_err(|err| {
                    LockError::Corrupt(format!("invalidation scope names {raw:?}: {err}"))
                })?)
            }
            other => return corrupt(format!("invalidation scope tag {other} is not 0, 1, or 2")),
        };
        let created_at = timestamp_from(tables, r.u()?)?;
        let reason_text = take_string(r, tables)?;
        let reason = Reason::parse(&reason_text).map_err(|err| {
            LockError::Corrupt(format!(
                "invalidation {previous_id} has an invalid reason: {err}"
            ))
        })?;
        fn documents(
            r: &mut Reader<'_>,
            tables: &mut DecodedTables,
            ids: Vec<u64>,
        ) -> Result<Vec<DocumentId>, LockError> {
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                let raw = charged_path(r, tables, id)?;
                out.push(DocumentId::parse(&raw).map_err(|err| {
                    LockError::Corrupt(format!("invalidation names document {raw:?}: {err}"))
                })?);
            }
            out.sort();
            Ok(out)
        }
        let target_ids = take_vector(r, tables)?;
        let pending_ids = take_vector(r, tables)?;
        let targets = documents(r, tables, target_ids)?;
        let pending = documents(r, tables, pending_ids)?;
        if pending.is_empty() {
            return corrupt(format!(
                "invalidation {previous_id} has no pending documents; completed invalidations leave the active list"
            ));
        }
        out.push(Invalidation {
            id: previous_id,
            scope,
            reason,
            created_at,
            targets,
            pending,
        });
    }
    Ok(out)
}
