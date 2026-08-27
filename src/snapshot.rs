//! Fail-closed persistent snapshots for complete buffered command results.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::ProcessOutput;
use crate::cli::ParseOutcome;
use crate::model::Selection;

const SNAPSHOT_VERSION: u32 = 2;
const PREPARATION_VERSION: u32 = 2;
const CACHE_DIRECTORY: &str = "cargo-sloc";
const ROOT_TRAVERSAL_POLICY_VERSION: u32 = 1;
const IGNORE_POLICY_VERSION: u32 = 1;
const PHYSICAL_IDENTITY_POLICY_VERSION: u32 = 1;
const ELIGIBILITY_POLICY_VERSION: u32 = 1;
const OWNERSHIP_POLICY_VERSION: u32 = 1;
const ROUTING_POLICY_VERSION: u32 = 1;
const RUST_ACCOUNTING_POLICY_VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
struct SnapshotRecord {
    version: u32,
    selection_key: String,
    input_fingerprint: String,
    output: ProcessOutput,
    integrity: String,
}

#[derive(Deserialize, Serialize)]
struct PreparationRecord {
    version: u32,
    selection_key: String,
    preparation_fingerprint: String,
    prepared: crate::app::PreparedExecution,
    integrity: String,
}

#[derive(Serialize)]
struct SelectionRecord<'a> {
    root: &'a str,
    package_selectors: Vec<&'a str>,
    workspace: bool,
    package_exclude_selectors: Vec<&'a str>,
    root_files: &'static str,
    all_features: bool,
    no_default_features: bool,
    features: Vec<&'a str>,
    requested_targets: Vec<&'a str>,
    target_includes: Vec<&'a str>,
    target_excludes: Vec<&'a str>,
    json: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResidentManifest {
    preparation_fingerprint: String,
    fingerprint: String,
    entries: BTreeMap<PathBuf, ResidentEntry>,
}

struct ResidentValidation {
    manifest: ResidentManifest,
    unchanged: BTreeSet<PathBuf>,
    changed_bytes: BTreeMap<PathBuf, Arc<[u8]>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResidentEntry {
    kind: ResidentEntryKind,
    stamp: Option<FileStamp>,
    link_target: Option<PathBuf>,
    resolved_stamp: Option<FileStamp>,
    content_digest: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResidentEntryKind {
    Missing,
    File,
    Directory,
    SymlinkFile,
    SymlinkDirectory,
    SymlinkOther,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    len: u64,
    modified_nanos: Option<u128>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanos: i64,
}

impl ResidentEntry {
    fn missing() -> Self {
        Self {
            kind: ResidentEntryKind::Missing,
            stamp: None,
            link_target: None,
            resolved_stamp: None,
            content_digest: None,
        }
    }
}

/// Resident report engine for repeated requests with one fixed selection.
///
/// A session validates the full input fingerprint before every refresh. Clean
/// refreshes return buffered bytes immediately. Source-only changes reuse the
/// prepared Cargo and toolchain state, while configuration changes rebuild it.
pub struct ResidentSession {
    selection: Selection,
    prepared: Option<crate::app::PreparedExecution>,
    source_cache: crate::rust_source::SourceCache,
    generic_source_cache: crate::generic_source::SourceCache,
    generic_accounting_cache: crate::tokei_accounting::AccountingCache,
    preparation_fingerprint: Option<String>,
    input_manifest: Option<ResidentManifest>,
    output: Option<ProcessOutput>,
}

impl ResidentSession {
    /// Parses one fixed request and creates an unprimed resident session.
    pub fn new<I, T>(arguments: I) -> Result<Self, ProcessOutput>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString>,
    {
        let current_directory = std::env::current_dir().map_err(|error| {
            crate::app::operational_error(crate::error::AppError::CurrentDirectory(error))
        })?;
        match crate::cli::parse(arguments, &current_directory) {
            Ok(ParseOutcome::Selection(selection)) => Ok(Self::from_selection(selection)),
            Ok(ParseOutcome::EarlyExit {
                stdout,
                stderr,
                exit_code,
            }) => Err(ProcessOutput {
                stdout,
                stderr,
                exit_code,
            }),
            Err(crate::error::AppError::Usage(message)) => Err(ProcessOutput {
                stdout: Vec::new(),
                stderr: format!("error: {message}\n\nFor more information, try '--help'.\n")
                    .into_bytes(),
                exit_code: 2,
            }),
            Err(error) => Err(crate::app::operational_error(error)),
        }
    }

    /// Validates state and returns the current deterministic report.
    pub fn refresh(&mut self) -> ProcessOutput {
        self.refresh_inner(true)
    }

    fn refresh_inner(&mut self, retry_on_race: bool) -> ProcessOutput {
        let cache_root = cache_root();
        let dependencies = self
            .source_cache
            .dependencies()
            .union(self.generic_source_cache.dependencies())
            .cloned()
            .collect();
        let input = match resident_manifest(
            &self.selection,
            &cache_root,
            self.input_manifest.as_ref(),
            &dependencies,
        ) {
            Ok(input) => input,
            Err(_) => {
                crate::metrics::record_snapshot_miss("resident-validation-error");
                return crate::app::execute(self.selection.clone());
            }
        };
        if self
            .input_manifest
            .as_ref()
            .is_some_and(|manifest| manifest.fingerprint == input.manifest.fingerprint)
            && let Some(output) = &self.output
        {
            crate::metrics::record_snapshot_hit("resident-validated");
            return output.clone();
        }

        let preparation = input.manifest.preparation_fingerprint.clone();
        let reuse_prepared = self.preparation_fingerprint.as_deref() == Some(preparation.as_str());
        let prepared = if reuse_prepared {
            crate::metrics::record_snapshot_miss("resident-source-changed");
            self.prepared.clone()
        } else {
            crate::metrics::record_snapshot_miss(if self.prepared.is_some() {
                "resident-configuration-changed"
            } else {
                "resident-unprimed"
            });
            match crate::app::prepare(&self.selection) {
                Ok(prepared) => Some(prepared),
                Err(error) => return crate::app::operational_error(error),
            }
        };
        let Some(prepared) = prepared else {
            return crate::app::execute(self.selection.clone());
        };
        self.source_cache
            .set_validation(input.unchanged.clone(), input.changed_bytes.clone());
        self.generic_source_cache
            .set_validation(input.unchanged, input.changed_bytes);
        let output = crate::app::execute_prepared_with_cache(
            self.selection.clone(),
            prepared.clone(),
            &mut self.source_cache,
            &mut self.generic_source_cache,
            &mut self.generic_accounting_cache,
        );
        if output.exit_code != 0 {
            return output;
        }
        let dependencies = self
            .source_cache
            .dependencies()
            .union(self.generic_source_cache.dependencies())
            .cloned()
            .collect();
        let Ok(after) = resident_manifest(
            &self.selection,
            &cache_root,
            Some(&input.manifest),
            &dependencies,
        ) else {
            return output;
        };
        if after.manifest.fingerprint != input.manifest.fingerprint {
            crate::metrics::record_snapshot_miss("resident-inputs-raced");
            if retry_on_race {
                self.input_manifest = Some(after.manifest);
                return self.refresh_inner(false);
            }
            return inputs_unstable_output();
        }

        self.prepared = Some(prepared);
        self.preparation_fingerprint = Some(preparation);
        self.input_manifest = Some(after.manifest);
        self.output = Some(output.clone());
        output
    }

    /// Refreshes the session while collecting opt-in pipeline metrics.
    pub fn refresh_with_metrics(&mut self) -> crate::metrics::MeasuredRun {
        crate::metrics::capture(|| self.refresh())
    }

    fn from_selection(selection: Selection) -> Self {
        Self {
            selection,
            prepared: None,
            source_cache: crate::rust_source::SourceCache::default(),
            generic_source_cache: crate::generic_source::SourceCache::default(),
            generic_accounting_cache: crate::tokei_accounting::AccountingCache::default(),
            preparation_fingerprint: None,
            input_manifest: None,
            output: None,
        }
    }
}

pub(crate) fn run(selection: Selection) -> ProcessOutput {
    let cache_root = cache_root();
    run_with_cache(selection, &cache_root)
}

fn run_with_cache(selection: Selection, cache_root: &Path) -> ProcessOutput {
    let selection_key = match selection_key(&selection) {
        Ok(key) => key,
        Err(_) => {
            crate::metrics::record_snapshot_miss("unsupported-selection");
            return crate::app::execute(selection);
        }
    };
    let fingerprint = match input_fingerprint(&selection, cache_root) {
        Ok(fingerprint) => fingerprint,
        Err(_) => {
            crate::metrics::record_snapshot_miss("validation-error");
            return crate::app::execute(selection);
        }
    };
    let path = match snapshot_path(&selection, &selection_key, cache_root) {
        Ok(path) => path,
        Err(_) => {
            crate::metrics::record_snapshot_miss("cache-unavailable");
            return crate::app::execute(selection);
        }
    };

    if let Some(output) = load(&path, &selection_key, &fingerprint) {
        if input_fingerprint(&selection, cache_root).is_ok_and(|current| current == fingerprint) {
            crate::metrics::record_snapshot_hit("validated");
            return output;
        }
        crate::metrics::record_snapshot_miss("inputs-raced");
    }

    let mut preparation = match preparation_fingerprint(&selection) {
        Ok(fingerprint) => fingerprint,
        Err(_) => return crate::app::execute(selection),
    };
    let preparation_path = match preparation_path(&selection, &selection_key, cache_root) {
        Ok(path) => path,
        Err(_) => return crate::app::execute(selection),
    };
    let prepared = if let Some(prepared) =
        load_preparation(&preparation_path, &selection_key, &preparation)
    {
        prepared
    } else {
        let mut prepared = match crate::app::prepare(&selection) {
            Ok(prepared) => prepared,
            Err(error) => return crate::app::operational_error(error),
        };
        let current = preparation_fingerprint(&selection).ok();
        if current.as_deref() != Some(preparation.as_str())
            && let Some(current) = current
        {
            prepared = match crate::app::prepare(&selection) {
                Ok(prepared) => prepared,
                Err(error) => return crate::app::operational_error(error),
            };
            preparation = current;
        }
        if preparation_fingerprint(&selection).is_ok_and(|current| current == preparation)
            && let Ok(integrity) = preparation_integrity(&selection_key, &preparation, &prepared)
        {
            let record = PreparationRecord {
                version: PREPARATION_VERSION,
                selection_key: selection_key.clone(),
                preparation_fingerprint: preparation.clone(),
                integrity,
                prepared: prepared.clone(),
            };
            if store_preparation(&preparation_path, &record).is_ok() {
                crate::metrics::record_preparation_write();
            }
        }
        prepared
    };
    let mut output = crate::app::execute_prepared(selection.clone(), prepared.clone());
    if output.exit_code != 0 {
        return output;
    }
    let Ok(mut after) = input_fingerprint(&selection, cache_root) else {
        return output;
    };
    if after != fingerprint {
        crate::metrics::record_snapshot_miss("inputs-changed-during-run");
        output = if preparation_fingerprint(&selection).is_ok_and(|current| current == preparation)
        {
            crate::app::execute_prepared(selection.clone(), prepared)
        } else {
            crate::app::execute(selection.clone())
        };
        if output.exit_code != 0 {
            return output;
        }
        let Ok(stable) = input_fingerprint(&selection, cache_root) else {
            return output;
        };
        if stable != after {
            crate::metrics::record_snapshot_miss("inputs-unstable");
            return inputs_unstable_output();
        }
        after = stable;
    }
    let integrity = integrity(&selection_key, &after, &output);
    let record = SnapshotRecord {
        version: SNAPSHOT_VERSION,
        selection_key,
        input_fingerprint: after,
        output: output.clone(),
        integrity,
    };
    if store(&path, &record).is_ok() {
        crate::metrics::record_snapshot_write();
    }
    output
}

fn inputs_unstable_output() -> ProcessOutput {
    crate::app::operational_error(crate::error::AppError::SnapshotInputsUnstable)
}

fn load(path: &Path, selection_key: &str, fingerprint: &str) -> Option<ProcessOutput> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            crate::metrics::record_snapshot_miss("absent");
            return None;
        }
        Err(_) => {
            crate::metrics::record_snapshot_miss("read-error");
            return None;
        }
    };
    let record: SnapshotRecord = match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        Err(_) => {
            crate::metrics::record_snapshot_miss("corrupt");
            return None;
        }
    };
    if record.version != SNAPSHOT_VERSION {
        crate::metrics::record_snapshot_miss("version");
        return None;
    }
    if record.selection_key != selection_key {
        crate::metrics::record_snapshot_miss("selection");
        return None;
    }
    if record.input_fingerprint != fingerprint {
        crate::metrics::record_snapshot_miss("inputs-changed");
        return None;
    }
    if record.integrity != integrity(selection_key, fingerprint, &record.output) {
        crate::metrics::record_snapshot_miss("integrity");
        return None;
    }
    Some(record.output)
}

fn store(path: &Path, record: &SnapshotRecord) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("snapshot path has no parent"))?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".{}.{}.tmp", std::process::id(), nonce));
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    let mut file = File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn load_preparation(
    path: &Path,
    selection_key: &str,
    fingerprint: &str,
) -> Option<crate::app::PreparedExecution> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            crate::metrics::record_preparation_miss("absent");
            return None;
        }
        Err(_) => {
            crate::metrics::record_preparation_miss("read-error");
            return None;
        }
    };
    let record: PreparationRecord = match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        Err(_) => {
            crate::metrics::record_preparation_miss("corrupt");
            return None;
        }
    };
    if record.version != PREPARATION_VERSION {
        crate::metrics::record_preparation_miss("version");
        return None;
    }
    if record.selection_key != selection_key {
        crate::metrics::record_preparation_miss("selection");
        return None;
    }
    if record.preparation_fingerprint != fingerprint {
        crate::metrics::record_preparation_miss("inputs-changed");
        return None;
    }
    if preparation_integrity(selection_key, fingerprint, &record.prepared)
        .ok()
        .as_deref()
        != Some(record.integrity.as_str())
    {
        crate::metrics::record_preparation_miss("integrity");
        return None;
    }
    crate::metrics::record_preparation_hit("validated");
    Some(record.prepared)
}

fn store_preparation(path: &Path, record: &PreparationRecord) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("preparation path has no parent"))?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".{}.{}.preparation.tmp", std::process::id(), nonce));
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    let mut file = File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn selection_key(selection: &Selection) -> io::Result<String> {
    let root = selection.root.as_path().to_str().ok_or_else(non_utf8)?;
    let record = SelectionRecord {
        root,
        package_selectors: strings(&selection.package_selectors),
        workspace: selection.workspace,
        package_exclude_selectors: strings(&selection.package_exclude_selectors),
        root_files: selection.root_files.as_str(),
        all_features: selection.all_features,
        no_default_features: selection.no_default_features,
        features: strings(&selection.features),
        requested_targets: strings(&selection.requested_targets),
        target_includes: strings(&selection.target_includes),
        target_excludes: strings(&selection.target_excludes),
        json: selection.json,
    };
    serde_json::to_string(&record).map_err(io::Error::other)
}

fn strings(values: &BTreeSet<String>) -> Vec<&str> {
    values.iter().map(String::as_str).collect()
}

fn snapshot_path(
    selection: &Selection,
    selection_key: &str,
    cache_root: &Path,
) -> io::Result<PathBuf> {
    let request = digest([selection_key.as_bytes()]);
    Ok(snapshot_project_root(selection, cache_root)?.join(format!("{request}.json")))
}

fn preparation_path(
    selection: &Selection,
    selection_key: &str,
    cache_root: &Path,
) -> io::Result<PathBuf> {
    let request = digest([selection_key.as_bytes()]);
    Ok(snapshot_project_root(selection, cache_root)?.join(format!("{request}.preparation.json")))
}

fn snapshot_project_root(selection: &Selection, cache_root: &Path) -> io::Result<PathBuf> {
    let root = selection.root.as_path().to_str().ok_or_else(non_utf8)?;
    Ok(cache_root.join(digest([root.as_bytes()])))
}

pub(crate) fn cache_root() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_SLOC_CACHE_DIR") {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache")
        .join(CACHE_DIRECTORY)
}

fn input_fingerprint(selection: &Selection, cache_root: &Path) -> io::Result<String> {
    let mut state = Digest::new();
    state.add(b"cargo-sloc-input-v1");
    state.add(env!("CARGO_PKG_VERSION").as_bytes());
    state.add(compatibility_digest().as_bytes());
    hash_selection(selection, &mut state)?;
    let mut visited = BTreeSet::new();
    let snapshot_root = snapshot_project_root(selection, cache_root)?;
    hash_tree(
        selection.root.as_path(),
        selection.root.as_path(),
        &snapshot_root,
        &mut state,
        &mut visited,
    )?;
    hash_ancestor_configuration(selection.root.as_path(), &mut state, &mut visited)?;
    hash_requested_targets(selection, &mut state, &mut visited)?;
    hash_environment(&mut state)?;
    hash_toolchain_state(&mut state, &mut visited)?;
    Ok(state.finish())
}

fn resident_manifest(
    selection: &Selection,
    cache_root: &Path,
    previous: Option<&ResidentManifest>,
    dependencies: &BTreeSet<PathBuf>,
) -> io::Result<ResidentValidation> {
    let preparation_fingerprint = preparation_fingerprint(selection)?;
    let cache_storage = snapshot_project_root(selection, cache_root)?;
    let mut entries = BTreeMap::new();
    let mut changed_bytes = BTreeMap::new();
    let mut visited_directories = BTreeSet::new();
    scan_resident_path(
        selection.root.as_path(),
        &cache_storage,
        previous,
        dependencies,
        &mut entries,
        &mut changed_bytes,
        &mut visited_directories,
    )?;
    for dependency in dependencies {
        if !entries.contains_key(dependency) {
            scan_resident_path(
                dependency,
                &cache_storage,
                previous,
                dependencies,
                &mut entries,
                &mut changed_bytes,
                &mut visited_directories,
            )?;
        }
    }

    let unchanged = dependencies
        .iter()
        .filter(|path| {
            previous
                .and_then(|manifest| manifest.entries.get(*path))
                .is_some_and(|entry| entries.get(*path) == Some(entry))
        })
        .cloned()
        .collect();
    let fingerprint = resident_manifest_fingerprint(&preparation_fingerprint, &entries)?;
    Ok(ResidentValidation {
        manifest: ResidentManifest {
            preparation_fingerprint,
            fingerprint,
            entries,
        },
        unchanged,
        changed_bytes,
    })
}

fn scan_resident_path(
    path: &Path,
    cache_root: &Path,
    previous: Option<&ResidentManifest>,
    dependencies: &BTreeSet<PathBuf>,
    entries: &mut BTreeMap<PathBuf, ResidentEntry>,
    changed_bytes: &mut BTreeMap<PathBuf, Arc<[u8]>>,
    visited_directories: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    if path.starts_with(cache_root) {
        return Ok(());
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            entries.insert(path.to_path_buf(), ResidentEntry::missing());
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let stamp = FileStamp::from_metadata(&metadata);
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let link_target = fs::read_link(path)?;
        let resolved = fs::metadata(path).ok();
        let resolved_stamp = resolved.as_ref().map(FileStamp::from_metadata);
        if resolved.as_ref().is_some_and(fs::Metadata::is_file) {
            let mut entry = ResidentEntry {
                kind: ResidentEntryKind::SymlinkFile,
                stamp: Some(stamp),
                link_target: Some(link_target),
                resolved_stamp,
                content_digest: None,
            };
            attach_resident_file_content(path, previous, dependencies, &mut entry, changed_bytes)?;
            entries.insert(path.to_path_buf(), entry);
            return Ok(());
        }
        let is_directory = resolved.as_ref().is_some_and(fs::Metadata::is_dir);
        entries.insert(
            path.to_path_buf(),
            ResidentEntry {
                kind: if is_directory {
                    ResidentEntryKind::SymlinkDirectory
                } else {
                    ResidentEntryKind::SymlinkOther
                },
                stamp: Some(stamp),
                link_target: Some(link_target),
                resolved_stamp,
                content_digest: None,
            },
        );
        if is_directory {
            scan_resident_directory(
                path,
                cache_root,
                previous,
                dependencies,
                entries,
                changed_bytes,
                visited_directories,
            )?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        let mut entry = ResidentEntry {
            kind: ResidentEntryKind::File,
            stamp: Some(stamp),
            link_target: None,
            resolved_stamp: None,
            content_digest: None,
        };
        attach_resident_file_content(path, previous, dependencies, &mut entry, changed_bytes)?;
        entries.insert(path.to_path_buf(), entry);
    } else if metadata.is_dir() {
        entries.insert(
            path.to_path_buf(),
            ResidentEntry {
                kind: ResidentEntryKind::Directory,
                stamp: Some(stamp),
                link_target: None,
                resolved_stamp: None,
                content_digest: None,
            },
        );
        scan_resident_directory(
            path,
            cache_root,
            previous,
            dependencies,
            entries,
            changed_bytes,
            visited_directories,
        )?;
    } else {
        entries.insert(
            path.to_path_buf(),
            ResidentEntry {
                kind: ResidentEntryKind::Other,
                stamp: Some(stamp),
                link_target: None,
                resolved_stamp: None,
                content_digest: None,
            },
        );
    }
    Ok(())
}

fn scan_resident_directory(
    path: &Path,
    cache_root: &Path,
    previous: Option<&ResidentManifest>,
    dependencies: &BTreeSet<PathBuf>,
    entries: &mut BTreeMap<PathBuf, ResidentEntry>,
    changed_bytes: &mut BTreeMap<PathBuf, Arc<[u8]>>,
    visited_directories: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let identity = path.canonicalize()?;
    if !visited_directories.insert(identity) {
        return Ok(());
    }
    let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        if child.path().starts_with(cache_root) || excluded_directory(&child) {
            continue;
        }
        scan_resident_path(
            &child.path(),
            cache_root,
            previous,
            dependencies,
            entries,
            changed_bytes,
            visited_directories,
        )?;
    }
    Ok(())
}

fn attach_resident_file_content(
    path: &Path,
    previous: Option<&ResidentManifest>,
    dependencies: &BTreeSet<PathBuf>,
    entry: &mut ResidentEntry,
    changed_bytes: &mut BTreeMap<PathBuf, Arc<[u8]>>,
) -> io::Result<()> {
    if let Some(previous_entry) = previous.and_then(|manifest| manifest.entries.get(path))
        && reusable_resident_digest(previous_entry, entry)
        && let Some(content_digest) = &previous_entry.content_digest
    {
        entry.content_digest = Some(content_digest.clone());
        return Ok(());
    }
    let bytes = Arc::<[u8]>::from(fs::read(path)?);
    entry.content_digest = Some(digest([bytes.as_ref()]));
    if dependencies.contains(path) {
        changed_bytes.insert(path.to_path_buf(), bytes);
    }
    Ok(())
}

#[cfg(unix)]
fn reusable_resident_digest(previous: &ResidentEntry, current: &ResidentEntry) -> bool {
    previous.kind == current.kind
        && previous.stamp == current.stamp
        && previous.link_target == current.link_target
        && previous.resolved_stamp == current.resolved_stamp
}

#[cfg(not(unix))]
fn reusable_resident_digest(_previous: &ResidentEntry, _current: &ResidentEntry) -> bool {
    false
}

fn resident_manifest_fingerprint(
    preparation_fingerprint: &str,
    entries: &BTreeMap<PathBuf, ResidentEntry>,
) -> io::Result<String> {
    let mut state = Digest::new();
    state.add(b"cargo-sloc-resident-manifest-v1");
    state.add(compatibility_digest().as_bytes());
    state.add(preparation_fingerprint.as_bytes());
    for (path, entry) in entries {
        if entry.kind == ResidentEntryKind::Missing {
            continue;
        }
        hash_path(path, &mut state)?;
        state.add(&[entry.kind as u8]);
        hash_optional_stamp(entry.stamp.as_ref(), &mut state);
        if let Some(target) = &entry.link_target {
            hash_path(target, &mut state)?;
        }
        hash_optional_stamp(entry.resolved_stamp.as_ref(), &mut state);
        if let Some(content_digest) = &entry.content_digest {
            state.add(content_digest.as_bytes());
        }
    }
    Ok(state.finish())
}

fn hash_optional_stamp(stamp: Option<&FileStamp>, state: &mut Digest) {
    if let Some(stamp) = stamp {
        state.add(&stamp.len.to_le_bytes());
        state.add(&stamp.modified_nanos.unwrap_or(u128::MAX).to_le_bytes());
        #[cfg(unix)]
        {
            state.add(&stamp.device.to_le_bytes());
            state.add(&stamp.inode.to_le_bytes());
            state.add(&stamp.mode.to_le_bytes());
            state.add(&stamp.changed_seconds.to_le_bytes());
            state.add(&stamp.changed_nanos.to_le_bytes());
        }
    } else {
        state.add(b"missing-stamp");
    }
}

impl FileStamp {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                len: metadata.len(),
                modified_nanos,
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                changed_seconds: metadata.ctime(),
                changed_nanos: metadata.ctime_nsec(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: metadata.len(),
                modified_nanos,
            }
        }
    }
}

fn preparation_fingerprint(selection: &Selection) -> io::Result<String> {
    let mut state = Digest::new();
    state.add(b"cargo-sloc-preparation-v1");
    state.add(env!("CARGO_PKG_VERSION").as_bytes());
    state.add(compatibility_digest().as_bytes());
    hash_selection(selection, &mut state)?;
    let mut visited = BTreeSet::new();
    hash_preparation_tree(
        selection.root.as_path(),
        selection.root.as_path(),
        &mut state,
    )?;
    hash_ancestor_configuration(selection.root.as_path(), &mut state, &mut visited)?;
    hash_requested_targets(selection, &mut state, &mut visited)?;
    hash_environment(&mut state)?;
    hash_toolchain_state(&mut state, &mut visited)?;
    Ok(state.finish())
}

fn hash_preparation_tree(root: &Path, path: &Path, state: &mut Digest) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    if metadata.file_type().is_symlink() {
        if preparation_file(path) {
            hash_path(relative, state)?;
            hash_path(&fs::read_link(path)?, state)?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        if preparation_file(path) {
            hash_path(relative, state)?;
            hash_metadata(&metadata, state);
            hash_file(path, state)?;
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        if excluded_preparation_directory(&entry) {
            continue;
        }
        hash_preparation_tree(root, &entry.path(), state)?;
    }
    Ok(())
}

fn preparation_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(OsStr::to_str),
        Some("Cargo.toml" | "Cargo.lock" | "config" | "config.toml" | ".gitignore" | ".ignore")
    ) || path.extension().and_then(OsStr::to_str) == Some("json")
}

fn hash_selection(selection: &Selection, state: &mut Digest) -> io::Result<()> {
    state.add(selection_key(selection)?.as_bytes());
    Ok(())
}

fn hash_tree(
    root: &Path,
    path: &Path,
    cache_root: &Path,
    state: &mut Digest,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    hash_path(relative, state)?;
    hash_metadata(&metadata, state);
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        hash_path(&target, state)?;
        if path.is_file()
            && let Ok(canonical) = path.canonicalize()
            && visited.insert(canonical.clone())
        {
            hash_external(&canonical, state, visited)?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        return hash_file(path, state);
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        if entry.path().starts_with(cache_root) {
            continue;
        }
        if excluded_directory(&entry) {
            continue;
        }
        hash_tree(root, &entry.path(), cache_root, state, visited)?;
    }
    Ok(())
}

fn hash_external(
    path: &Path,
    state: &mut Digest,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    hash_path(path, state)?;
    hash_metadata(&metadata, state);
    if metadata.is_file() {
        hash_file(path, state)?;
    } else if metadata.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if visited.insert(canonical) {
                hash_external(&path, state, visited)?;
            }
        }
    }
    Ok(())
}

fn excluded_directory(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && matches!(entry.file_name().to_str(), Some(".git" | ".cargo-sloc"))
}

fn excluded_preparation_directory(entry: &fs::DirEntry) -> bool {
    excluded_directory(entry)
        || (entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry.file_name() == OsStr::new("target"))
}

fn hash_file(path: &Path, state: &mut Digest) -> io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        state.add(&buffer[..read]);
    }
    Ok(())
}

fn hash_metadata(metadata: &fs::Metadata, state: &mut Digest) {
    if !metadata.is_dir() {
        state.add(&metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = modified.duration_since(UNIX_EPOCH)
        {
            state.add(&elapsed.as_secs().to_le_bytes());
            state.add(&elapsed.subsec_nanos().to_le_bytes());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        state.add(&metadata.mode().to_le_bytes());
        state.add(&metadata.ino().to_le_bytes());
    }
}

fn hash_ancestor_configuration(
    root: &Path,
    state: &mut Digest,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    for ancestor in root.ancestors() {
        for path in [
            ancestor.join(".cargo/config.toml"),
            ancestor.join(".cargo/config"),
        ] {
            if path.is_file() {
                hash_external_once(&path, state, visited)?;
                hash_referenced_json(&path, state, visited)?;
            }
        }
    }
    if let Some(home) = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
    {
        for path in [home.join("config.toml"), home.join("config")] {
            if path.is_file() {
                hash_external_once(&path, state, visited)?;
                hash_referenced_json(&path, state, visited)?;
            }
        }
    }
    Ok(())
}

fn hash_referenced_json(
    config: &Path,
    state: &mut Digest,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let contents = fs::read_to_string(config)?;
    for quoted in contents
        .split(['\'', '"'])
        .filter(|value| value.ends_with(".json"))
    {
        let path = config.parent().unwrap_or(Path::new(".")).join(quoted);
        if path.is_file() {
            hash_external_once(&path, state, visited)?;
        }
    }
    Ok(())
}

fn hash_requested_targets(
    selection: &Selection,
    state: &mut Digest,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    for target in &selection.requested_targets {
        let path = Path::new(target);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            selection.root.as_path().join(path)
        };
        if path.is_file() {
            hash_external_once(&path, state, visited)?;
        }
    }
    Ok(())
}

fn hash_environment(state: &mut Digest) -> io::Result<()> {
    let mut variables = std::env::vars_os()
        .filter(|(name, _)| relevant_environment(name))
        .collect::<Vec<_>>();
    variables.sort();
    hash_environment_values(variables, state)
}

fn hash_environment_values(
    variables: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    state: &mut Digest,
) -> io::Result<()> {
    for (name, value) in variables {
        hash_os(&name, state)?;
        hash_os(&value, state)?;
    }
    Ok(())
}

fn relevant_environment(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with("CARGO_")
        || name.starts_with("RUST")
        || matches!(
            name.as_ref(),
            "PATH" | "HOME" | "HOST" | "TARGET" | "CC" | "CFLAGS"
        )
}

fn hash_toolchain_state(state: &mut Digest, visited: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    for (variable, fallback) in [
        ("CARGO", Some("cargo")),
        ("RUSTC", Some("rustc")),
        ("RUSTC_WRAPPER", None),
        ("RUSTC_WORKSPACE_WRAPPER", None),
    ] {
        let command = std::env::var_os(variable).or_else(|| fallback.map(Into::into));
        if let Some(path) = command.as_deref().and_then(resolve_command) {
            hash_executable_identity(&path, state)?;
        }
    }
    if let Some(home) = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")))
    {
        let settings = home.join("settings.toml");
        if settings.is_file() {
            hash_external_once(&settings, state, visited)?;
        }
    }
    Ok(())
}

fn hash_executable_identity(path: &Path, state: &mut Digest) -> io::Result<()> {
    hash_path(path, state)?;
    let metadata = fs::symlink_metadata(path)?;
    hash_metadata(&metadata, state);
    if metadata.file_type().is_symlink() {
        hash_path(&fs::read_link(path)?, state)?;
    }
    let canonical = path.canonicalize()?;
    hash_path(&canonical, state)?;
    hash_metadata(&fs::metadata(canonical)?, state);
    Ok(())
}

fn resolve_command(command: &OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(command);
    if path.components().count() > 1 {
        return path.is_file().then_some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(&path);
        candidate.is_file().then_some(candidate)
    })
}

fn hash_external_once(
    path: &Path,
    state: &mut Digest,
    visited: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    let canonical = path.canonicalize()?;
    if visited.insert(canonical.clone()) {
        hash_external(&canonical, state, visited)?;
    }
    Ok(())
}

fn integrity(selection_key: &str, fingerprint: &str, output: &ProcessOutput) -> String {
    digest([
        SNAPSHOT_VERSION.to_string().as_bytes(),
        selection_key.as_bytes(),
        fingerprint.as_bytes(),
        &output.stdout,
        &output.stderr,
        &[output.exit_code],
    ])
}

fn preparation_integrity(
    selection_key: &str,
    fingerprint: &str,
    prepared: &crate::app::PreparedExecution,
) -> io::Result<String> {
    let prepared = serde_json::to_vec(prepared).map_err(io::Error::other)?;
    Ok(digest([
        PREPARATION_VERSION.to_string().as_bytes(),
        selection_key.as_bytes(),
        fingerprint.as_bytes(),
        &prepared,
    ]))
}

#[derive(Clone, Copy)]
struct CompatibilityVersions<'a> {
    json_schema: u8,
    tokei_catalog: &'a str,
    tokei_adapter: u32,
    inventory: u32,
    root_traversal: u32,
    ignore: u32,
    physical_identity: u32,
    eligibility: u32,
    ownership: u32,
    routing: u32,
    rust_accounting: u32,
}

fn compatibility_digest() -> String {
    compatibility_digest_for(CompatibilityVersions {
        json_schema: crate::report::JSON_SCHEMA_VERSION,
        tokei_catalog: crate::tokei_accounting::CATALOG_VERSION,
        tokei_adapter: crate::tokei_accounting::ADAPTER_VERSION,
        inventory: crate::generic_source::INVENTORY_POLICY_VERSION,
        root_traversal: ROOT_TRAVERSAL_POLICY_VERSION,
        ignore: IGNORE_POLICY_VERSION,
        physical_identity: PHYSICAL_IDENTITY_POLICY_VERSION,
        eligibility: ELIGIBILITY_POLICY_VERSION,
        ownership: OWNERSHIP_POLICY_VERSION,
        routing: ROUTING_POLICY_VERSION,
        rust_accounting: RUST_ACCOUNTING_POLICY_VERSION,
    })
}

fn compatibility_digest_for(versions: CompatibilityVersions<'_>) -> String {
    digest([
        b"cargo-sloc-compatibility-v1".as_slice(),
        &[versions.json_schema],
        versions.tokei_catalog.as_bytes(),
        &versions.tokei_adapter.to_le_bytes(),
        &versions.inventory.to_le_bytes(),
        &versions.root_traversal.to_le_bytes(),
        &versions.ignore.to_le_bytes(),
        &versions.physical_identity.to_le_bytes(),
        &versions.eligibility.to_le_bytes(),
        &versions.ownership.to_le_bytes(),
        &versions.routing.to_le_bytes(),
        &versions.rust_accounting.to_le_bytes(),
    ])
}

fn digest<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut state = Digest::new();
    for part in parts {
        state.add(part);
    }
    state.finish()
}

struct Digest {
    first: u64,
    second: u64,
}

impl Digest {
    fn new() -> Self {
        Self {
            first: 0xcbf2_9ce4_8422_2325,
            second: 0x8422_2325_cbf2_9ce4,
        }
    }

    fn add(&mut self, bytes: &[u8]) {
        self.first ^= u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.second ^= self.first.rotate_left(17);
        for byte in bytes {
            self.first ^= u64::from(*byte);
            self.first = self.first.wrapping_mul(0x0000_0100_0000_01b3);
            self.second ^= u64::from(*byte).wrapping_add(self.first.rotate_left(23));
            self.second = self.second.wrapping_mul(0x9e37_79b1_85eb_ca87);
        }
    }

    fn finish(self) -> String {
        format!("{:016x}{:016x}", self.first, self.second)
    }
}

fn hash_path(path: &Path, state: &mut Digest) -> io::Result<()> {
    hash_os(path.as_os_str(), state)
}

fn hash_os(value: &OsStr, state: &mut Digest) -> io::Result<()> {
    let value = value.to_str().ok_or_else(non_utf8)?;
    state.add(value.as_bytes());
    Ok(())
}

fn non_utf8() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "snapshot input is not UTF-8")
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsString, OsString as TestOsString};

    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;
    use crate::cli::ParseOutcome;

    #[test]
    fn validated_snapshot_hits_and_source_changes_invalidate() {
        let root = package("snapshot-hit");
        let cache = root.path().join(".cargo-sloc");
        let selection = selection(root.path());

        let cold = measured(|| run_with_cache(selection.clone(), &cache));
        assert_eq!(cold.output.exit_code, 0);
        assert_eq!(cold.metrics.caches.snapshot_hits, 0);
        assert_eq!(cold.metrics.caches.snapshot_writes, 1);

        let warm = measured(|| run_with_cache(selection.clone(), &cache));
        assert_eq!(warm.output, cold.output);
        assert_eq!(warm.metrics.caches.snapshot_hits, 1);
        assert_eq!(warm.metrics.subprocesses, 0);
        assert_eq!(warm.metrics.workload.projects, 0);

        fs::write(
            root.path().join("src/lib.rs"),
            "pub fn original() {}\npub fn edited() {}\n",
        )
        .expect("edit snapshot source");
        let edited = measured(|| run_with_cache(selection.clone(), &cache));
        assert_eq!(edited.output.exit_code, 0);
        assert_eq!(edited.metrics.caches.snapshot_hits, 0);
        assert!(edited.metrics.caches.snapshot_misses >= 1);
        assert_eq!(edited.metrics.caches.preparation_hits, 1);
        assert_eq!(edited.metrics.subprocesses, 0);
        assert_ne!(edited.output.stdout, warm.output.stdout);

        let edited_warm = measured(|| run_with_cache(selection, &cache));
        assert_eq!(edited_warm.output, edited.output);
        assert_eq!(edited_warm.metrics.caches.snapshot_hits, 1);
        assert_eq!(edited_warm.metrics.subprocesses, 0);
    }

    #[test]
    fn target_source_changes_invalidate_persistent_snapshots() {
        let root = package("snapshot-target-source");
        let cache = tempfile::tempdir().expect("create snapshot cache");
        fs::create_dir_all(root.path().join("target")).expect("create target directory");
        fs::write(
            root.path().join("target/generated.js"),
            "const generated = true;\n",
        )
        .expect("write generated target source");
        let selection = selection(root.path());

        let cold = measured(|| run_with_cache(selection.clone(), cache.path()));
        assert_eq!(cold.output.exit_code, 0);
        let warm = measured(|| run_with_cache(selection.clone(), cache.path()));
        assert_eq!(warm.output, cold.output);
        assert_eq!(warm.metrics.caches.snapshot_hits, 1);

        fs::write(
            root.path().join("target/generated.js"),
            "const generated = true;\nconst edited = true;\n",
        )
        .expect("edit generated target source");
        let edited = measured(|| run_with_cache(selection.clone(), cache.path()));

        assert_eq!(edited.output.exit_code, 0);
        assert_eq!(edited.metrics.caches.snapshot_hits, 0);
        assert_eq!(edited.metrics.caches.preparation_hits, 1);
        assert_eq!(edited.metrics.subprocesses, 0);
        assert_ne!(edited.output.stdout, warm.output.stdout);
        assert_eq!(edited.output, crate::app::execute(selection));
    }

    #[test]
    fn default_cache_root_is_under_home_cache() {
        let cache = cache_root();
        assert_eq!(cache.file_name(), Some(OsStr::new(CACHE_DIRECTORY)));
        assert_eq!(
            cache.parent().and_then(Path::file_name),
            Some(OsStr::new(".cache"))
        );
    }

    #[test]
    fn manifest_lock_config_and_ignore_changes_invalidate() {
        let root = package("snapshot-inputs");
        let cache = tempfile::tempdir().expect("create snapshot cache");
        let selection = selection(root.path());
        let first = run_with_cache(selection.clone(), cache.path());
        assert_eq!(first.exit_code, 0);

        for (path, contents) in [
            ("Cargo.toml", "\n# snapshot manifest change\n"),
            ("Cargo.lock", "\n# snapshot lock change\n"),
            (".cargo/config.toml", "# snapshot config change\n"),
            (".gitignore", "# snapshot ignore change\n"),
        ] {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().expect("input parent")).expect("create input parent");
            let mut previous = fs::read_to_string(&path).unwrap_or_default();
            previous.push_str(contents);
            fs::write(&path, previous).expect("mutate snapshot input");
            let changed = measured(|| run_with_cache(selection.clone(), cache.path()));
            assert_eq!(changed.output.exit_code, 0, "input {}", path.display());
            assert_eq!(
                changed.metrics.caches.snapshot_hits,
                0,
                "input {}",
                path.display()
            );
            assert!(changed.metrics.caches.snapshot_misses >= 1);
            let stable = measured(|| run_with_cache(selection.clone(), cache.path()));
            assert_eq!(
                stable.metrics.caches.snapshot_hits,
                1,
                "input {}",
                path.display()
            );
        }
    }

    #[test]
    fn corrupt_and_version_mismatched_records_fail_closed() {
        let root = package("snapshot-corruption");
        let cache = tempfile::tempdir().expect("create snapshot cache");
        let selection = selection(root.path());
        assert_eq!(run_with_cache(selection.clone(), cache.path()).exit_code, 0);
        let path = only_snapshot(cache.path());

        fs::write(&path, b"not json").expect("corrupt snapshot");
        let corrupt = measured(|| run_with_cache(selection.clone(), cache.path()));
        assert_eq!(corrupt.output.exit_code, 0);
        assert_eq!(
            corrupt.metrics.caches.outcomes.get("snapshot.miss.corrupt"),
            Some(&1)
        );
        assert_eq!(corrupt.metrics.caches.preparation_hits, 1);
        assert_eq!(corrupt.metrics.subprocesses, 0);

        let mut record: Value = serde_json::from_slice(&fs::read(&path).expect("read snapshot"))
            .expect("parse rewritten snapshot");
        record["version"] = Value::from(SNAPSHOT_VERSION + 1);
        fs::write(
            &path,
            serde_json::to_vec(&record).expect("serialize version mismatch"),
        )
        .expect("write version mismatch");
        let version = measured(|| run_with_cache(selection, cache.path()));
        assert_eq!(version.output.exit_code, 0);
        assert_eq!(
            version.metrics.caches.outcomes.get("snapshot.miss.version"),
            Some(&1)
        );
        assert_eq!(version.metrics.caches.preparation_hits, 1);
        assert_eq!(version.metrics.subprocesses, 0);
    }

    #[test]
    fn corrupt_and_version_mismatched_preparation_records_fail_closed() {
        let root = package("preparation-corruption");
        let cache = tempfile::tempdir().expect("create preparation cache");
        let selection = selection(root.path());
        assert_eq!(run_with_cache(selection.clone(), cache.path()).exit_code, 0);
        let path = only_preparation(cache.path());

        fs::write(&path, b"not json").expect("corrupt preparation record");
        fs::write(
            root.path().join("src/lib.rs"),
            "mod unchanged;\npub fn original() {}\npub fn first_edit() {}\n",
        )
        .expect("invalidate rendered snapshot after preparation corruption");
        let corrupt = measured(|| run_with_cache(selection.clone(), cache.path()));
        assert_eq!(corrupt.output.exit_code, 0);
        assert_eq!(
            corrupt
                .metrics
                .caches
                .outcomes
                .get("preparation.miss.corrupt"),
            Some(&1)
        );
        assert!(corrupt.metrics.subprocesses > 0);

        let mut record: Value = serde_json::from_slice(&fs::read(&path).expect("read preparation"))
            .expect("parse rewritten preparation");
        record["version"] = Value::from(PREPARATION_VERSION + 1);
        fs::write(
            &path,
            serde_json::to_vec(&record).expect("serialize preparation version mismatch"),
        )
        .expect("write preparation version mismatch");
        fs::write(
            root.path().join("src/lib.rs"),
            "mod unchanged;\npub fn original() {}\npub fn second_edit() {}\n",
        )
        .expect("invalidate rendered snapshot after preparation version mismatch");
        let version = measured(|| run_with_cache(selection, cache.path()));
        assert_eq!(version.output.exit_code, 0);
        assert_eq!(
            version
                .metrics
                .caches
                .outcomes
                .get("preparation.miss.version"),
            Some(&1)
        );
        assert!(version.metrics.subprocesses > 0);
    }

    #[test]
    fn environment_target_and_external_symlink_inputs_change_fingerprints() {
        let root = package("snapshot-fingerprint");
        let cache = tempfile::tempdir().expect("create snapshot cache");
        let mut selection = selection(root.path());

        let mut first = Digest::new();
        hash_environment_values(
            vec![(
                TestOsString::from("RUSTFLAGS"),
                TestOsString::from("--cfg one"),
            )],
            &mut first,
        )
        .expect("hash first environment");
        let mut second = Digest::new();
        hash_environment_values(
            vec![(
                TestOsString::from("RUSTFLAGS"),
                TestOsString::from("--cfg two"),
            )],
            &mut second,
        )
        .expect("hash second environment");
        assert_ne!(first.finish(), second.finish());

        let target = root.path().join("custom-target.json");
        fs::write(&target, "{\"llvm-target\":\"first\"}").expect("write target spec");
        selection
            .requested_targets
            .insert(target.to_string_lossy().into_owned());
        let before = input_fingerprint(&selection, cache.path()).expect("first target fingerprint");
        fs::write(&target, "{\"llvm-target\":\"second\"}").expect("edit target spec");
        let after = input_fingerprint(&selection, cache.path()).expect("second target fingerprint");
        assert_ne!(before, after);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let external = TempDir::new().expect("create external source");
            let external_file = external.path().join("external.rs");
            fs::write(&external_file, "pub fn first() {}\n").expect("write external source");
            symlink(&external_file, root.path().join("src/external.rs"))
                .expect("create source symlink");
            let before =
                input_fingerprint(&selection, cache.path()).expect("first symlink fingerprint");
            fs::write(&external_file, "pub fn second() {}\n").expect("edit external source");
            let after =
                input_fingerprint(&selection, cache.path()).expect("second symlink fingerprint");
            assert_ne!(before, after);

            let external_directory = TempDir::new().expect("create external directory");
            fs::write(
                external_directory.path().join("generated.rs"),
                "pub fn first() {}\n",
            )
            .expect("write external generated source");
            symlink(
                external_directory.path(),
                root.path().join("linked-directory"),
            )
            .expect("create directory symlink");
            let before = input_fingerprint(&selection, cache.path())
                .expect("first directory symlink fingerprint");
            fs::write(
                external_directory.path().join("generated.rs"),
                "pub fn second() {}\n",
            )
            .expect("edit external generated source");
            let after = input_fingerprint(&selection, cache.path())
                .expect("second directory symlink fingerprint");
            assert_eq!(before, after);
        }
    }

    #[test]
    fn resident_session_reuses_preparation_for_source_changes() {
        let root = package("resident-snapshot");
        let selection = selection(root.path());
        assert_eq!(crate::app::execute(selection.clone()).exit_code, 0);
        let mut session = ResidentSession::from_selection(selection.clone());

        let cold = session.refresh_with_metrics();
        assert_eq!(cold.output.exit_code, 0);
        assert!(cold.metrics.subprocesses > 0);

        let warm = session.refresh_with_metrics();
        assert_eq!(warm.output, cold.output);
        assert_eq!(warm.metrics.caches.snapshot_hits, 1);
        assert_eq!(warm.metrics.subprocesses, 0);
        assert_eq!(warm.metrics.workload.projects, 0);

        fs::write(
            root.path().join("src/lib.rs"),
            "mod unchanged;\npub fn original() {}\npub fn edited() {}\n",
        )
        .expect("edit resident source");
        let edited = session.refresh_with_metrics();
        assert_eq!(edited.output.exit_code, 0);
        assert_eq!(edited.metrics.subprocesses, 0);
        assert_eq!(edited.metrics.caches.parse_hits, 1);
        assert_eq!(edited.metrics.caches.parse_misses, 1);
        assert_eq!(edited.metrics.workload.file_analysis_lowerings, 1);
        assert_eq!(
            edited
                .metrics
                .caches
                .outcomes
                .get("parse.hit.validated-unchanged-source"),
            Some(&1)
        );
        assert_eq!(
            edited
                .metrics
                .caches
                .outcomes
                .get("snapshot.miss.resident-source-changed"),
            Some(&1)
        );
        assert_ne!(edited.output.stdout, warm.output.stdout);
        assert_eq!(edited.output, crate::app::execute(selection.clone()));

        let manifest = root.path().join("Cargo.toml");
        let mut contents = fs::read_to_string(&manifest).expect("read resident manifest");
        contents.push_str("\n# resident configuration change\n");
        fs::write(&manifest, contents).expect("edit resident manifest");
        let reconfigured = session.refresh_with_metrics();
        assert_eq!(reconfigured.output.exit_code, 0);
        assert!(reconfigured.metrics.subprocesses > 0);
        assert_eq!(
            reconfigured
                .metrics
                .caches
                .outcomes
                .get("snapshot.miss.resident-configuration-changed"),
            Some(&1)
        );
        assert_eq!(reconfigured.output, crate::app::execute(selection));
    }

    #[test]
    fn resident_session_reuses_unchanged_generic_bytes_and_lexical_results() {
        let root = package("resident-generic");
        fs::write(root.path().join("app.js"), "const original = true;\n")
            .expect("write generic source");
        fs::write(
            root.path().join("tool"),
            "#!/usr/bin/env python3\nprint('stable')\n",
        )
        .expect("write extensionless generic source");
        let selection = selection(root.path());
        let mut session = ResidentSession::from_selection(selection.clone());

        let cold = session.refresh_with_metrics();
        assert_eq!(cold.output.exit_code, 0);
        assert_eq!(cold.metrics.caches.generic_source_misses, 5);
        assert_eq!(cold.metrics.caches.generic_accounting_misses, 3);

        let warm = session.refresh_with_metrics();
        assert_eq!(warm.output, cold.output);
        assert_eq!(warm.metrics.caches.snapshot_hits, 1);

        fs::write(
            root.path().join("app.js"),
            "const original = true;\nconst edited = true;\n",
        )
        .expect("edit generic source");
        let edited = session.refresh_with_metrics();

        assert_eq!(edited.output.exit_code, 0);
        assert_eq!(edited.metrics.subprocesses, 0);
        assert_eq!(edited.metrics.caches.generic_source_hits, 4);
        assert_eq!(edited.metrics.caches.generic_source_misses, 1);
        assert_eq!(edited.metrics.caches.generic_accounting_hits, 2);
        assert_eq!(edited.metrics.caches.generic_accounting_misses, 1);
        assert_ne!(edited.output.stdout, warm.output.stdout);
        assert_eq!(edited.output, crate::app::execute(selection));
    }

    #[test]
    fn target_source_changes_invalidate_resident_snapshots() {
        let root = package("resident-target-source");
        fs::create_dir_all(root.path().join("target")).expect("create target directory");
        fs::write(
            root.path().join("target/generated.js"),
            "const generated = true;\n",
        )
        .expect("write generated target source");
        let selection = selection(root.path());
        let mut session = ResidentSession::from_selection(selection.clone());

        let cold = session.refresh_with_metrics();
        assert_eq!(cold.output.exit_code, 0);
        let warm = session.refresh_with_metrics();
        assert_eq!(warm.output, cold.output);
        assert_eq!(warm.metrics.caches.snapshot_hits, 1);

        fs::write(
            root.path().join("target/generated.js"),
            "const generated = true;\nconst edited = true;\n",
        )
        .expect("edit generated target source");
        let edited = session.refresh_with_metrics();

        assert_eq!(edited.output.exit_code, 0);
        assert_eq!(edited.metrics.caches.snapshot_hits, 0);
        assert_eq!(edited.metrics.subprocesses, 0);
        assert_eq!(
            edited
                .metrics
                .caches
                .outcomes
                .get("snapshot.miss.resident-source-changed"),
            Some(&1)
        );
        assert_ne!(edited.output.stdout, warm.output.stdout);
        assert_eq!(edited.output, crate::app::execute(selection));
    }

    #[test]
    fn every_generic_compatibility_input_changes_the_snapshot_digest() {
        let versions = CompatibilityVersions {
            json_schema: 3,
            tokei_catalog: "tokei-14.0.0",
            tokei_adapter: 1,
            inventory: 1,
            root_traversal: 1,
            ignore: 1,
            physical_identity: 1,
            eligibility: 1,
            ownership: 1,
            routing: 1,
            rust_accounting: 1,
        };
        let baseline = compatibility_digest_for(versions);

        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                json_schema: 4,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                tokei_catalog: "tokei-15.0.0",
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                tokei_adapter: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                inventory: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                root_traversal: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                ignore: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                physical_identity: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                eligibility: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                ownership: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                routing: 2,
                ..versions
            })
        );
        assert_ne!(
            baseline,
            compatibility_digest_for(CompatibilityVersions {
                rust_accounting: 2,
                ..versions
            })
        );
    }

    #[test]
    fn resident_manifest_tracks_deleted_and_renamed_sources_without_repreparing() {
        let root = package("resident-path-changes");
        let selection = selection(root.path());
        let mut session = ResidentSession::from_selection(selection.clone());
        let cold = session.refresh_with_metrics();
        assert_eq!(cold.output.exit_code, 0);

        fs::remove_file(root.path().join("src/unchanged.rs")).expect("delete resident source");
        let deleted = session.refresh_with_metrics();
        assert_eq!(deleted.output.exit_code, 1);
        assert_eq!(deleted.metrics.subprocesses, 0);
        assert!(deleted.output.stdout.is_empty());
        assert_eq!(deleted.output, crate::app::execute(selection.clone()));

        fs::write(root.path().join("src/renamed.rs"), "pub fn renamed() {}\n")
            .expect("write renamed resident source");
        fs::write(
            root.path().join("src/lib.rs"),
            "mod renamed;\npub fn original() {}\n",
        )
        .expect("point resident package at renamed source");
        let renamed = session.refresh_with_metrics();
        assert_eq!(renamed.output.exit_code, 0);
        assert_eq!(renamed.metrics.subprocesses, 0);
        assert_eq!(renamed.metrics.caches.parse_misses, 2);
        assert_eq!(renamed.metrics.workload.file_analysis_lowerings, 2);
        assert_eq!(renamed.output, crate::app::execute(selection.clone()));

        let stable = session.refresh_with_metrics();
        assert_eq!(stable.output, renamed.output);
        assert_eq!(stable.metrics.caches.snapshot_hits, 1);
        assert_eq!(stable.metrics.subprocesses, 0);
    }

    #[test]
    fn resident_manifest_scans_roots_nested_below_cache_root() {
        let cache = tempfile::tempdir().expect("create cache root");
        let root = cache.path().join("project");
        fs::create_dir_all(root.join("src")).expect("create project source directory");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"nested-cache-root\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write project manifest");
        fs::write(root.join("src/lib.rs"), "pub fn first() {}\n").expect("write source");
        let selection = selection(&root);

        let first = resident_manifest(&selection, cache.path(), None, &BTreeSet::new())
            .expect("scan nested Root");
        assert!(first.manifest.entries.contains_key(&root));
        assert!(
            first
                .manifest
                .entries
                .contains_key(&root.join("src/lib.rs"))
        );

        fs::write(root.join("src/lib.rs"), "pub fn second() {}\n").expect("edit source");
        let second = resident_manifest(
            &selection,
            cache.path(),
            Some(&first.manifest),
            &BTreeSet::new(),
        )
        .expect("rescan nested Root");
        assert_ne!(first.manifest.fingerprint, second.manifest.fingerprint);
    }

    #[test]
    fn resident_manifest_detects_new_module_resolution_candidates() {
        let root = package("resident-module-candidates");
        let selection = selection(root.path());
        let mut session = ResidentSession::from_selection(selection.clone());
        let cold = session.refresh_with_metrics();
        assert_eq!(cold.output.exit_code, 0);

        let nested = root.path().join("src/unchanged/mod.rs");
        fs::create_dir_all(nested.parent().expect("nested module parent"))
            .expect("create nested module directory");
        fs::write(&nested, "pub fn competing() {}\n").expect("write competing nested module");

        let ambiguous = session.refresh_with_metrics();
        assert_eq!(ambiguous.output.exit_code, 1);
        assert_eq!(ambiguous.metrics.subprocesses, 0);
        assert!(ambiguous.output.stdout.is_empty());
        assert_eq!(ambiguous.output, crate::app::execute(selection));
    }

    #[test]
    fn repeated_snapshot_input_instability_fails_closed() {
        let output = inputs_unstable_output();
        assert_eq!(output.exit_code, 1);
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("inputs remained unstable"));
    }

    fn measured(operation: impl FnOnce() -> ProcessOutput) -> crate::metrics::MeasuredRun {
        crate::metrics::capture(operation)
    }

    fn selection(root: &Path) -> Selection {
        match crate::cli::parse(
            [OsString::from("--json"), root.as_os_str().to_owned()],
            root,
        )
        .expect("parse snapshot selection")
        {
            ParseOutcome::Selection(selection) => selection,
            ParseOutcome::EarlyExit { .. } => panic!("unexpected snapshot parse exit"),
        }
    }

    fn package(name: &str) -> TempDir {
        let root = tempfile::tempdir().expect("create snapshot package");
        fs::create_dir_all(root.path().join("src")).expect("create source directory");
        fs::write(
            root.path().join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        )
        .expect("write snapshot manifest");
        fs::write(
            root.path().join("src/lib.rs"),
            "mod unchanged;\npub fn original() {}\n",
        )
        .expect("write snapshot source");
        fs::write(
            root.path().join("src/unchanged.rs"),
            "pub fn unchanged() {}\n",
        )
        .expect("write unchanged snapshot source");
        root
    }

    fn only_snapshot(root: &Path) -> PathBuf {
        let project = fs::read_dir(root)
            .expect("read cache root")
            .next()
            .expect("project cache")
            .expect("read project cache")
            .path();
        fs::read_dir(project)
            .expect("read project snapshots")
            .find(|entry| {
                entry.as_ref().is_ok_and(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".preparation.json")
                })
            })
            .expect("snapshot record")
            .expect("read snapshot record")
            .path()
    }

    fn only_preparation(root: &Path) -> PathBuf {
        let project = fs::read_dir(root)
            .expect("read cache root")
            .next()
            .expect("project cache")
            .expect("read project cache")
            .path();
        fs::read_dir(project)
            .expect("read project cache records")
            .find(|entry| {
                entry.as_ref().is_ok_and(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".preparation.json")
                })
            })
            .expect("preparation record")
            .expect("read preparation record")
            .path()
    }
}
