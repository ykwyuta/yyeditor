//! Explicit, journalled synchronization. Originals are moved aside, never deleted.
//! A run is recoverable, not a transaction spanning both trees or a power-loss guarantee.
use crate::{
    Cancellation, Result, folders,
    sync_plan::{self, Action, Plan},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    directory: bool,
    identity: String,
    length: u64,
    modified: String,
    permissions: u32,
    digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tree {
    root: PathBuf,
    entries: BTreeMap<PathBuf, Stamp>,
}
pub type Inventory = Vec<Tree>;

fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}
fn identity(path: &Path, meta: &fs::Metadata) -> Result<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        Ok(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(h: *mut std::ffi::c_void, info: *mut [u32; 13]) -> i32;
        }
        let _ = meta;
        let file = io(OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000 | 0x00200000)
            .open(path))?;
        let mut info = [0; 13];
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(format!("{}:{}:{}", info[7], info[11], info[12]))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, meta);
        Err("unsupported filesystem platform".into())
    }
}
fn permissions(m: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        u32::from(m.permissions().readonly())
    }
}
fn stamp(path: &Path, cancel: &Cancellation) -> Result<Stamp> {
    cancel.check()?;
    let m = io(fs::symlink_metadata(path))?;
    if folders::is_link(path, m.file_type())? || (!m.is_file() && !m.is_dir()) {
        return Err(format!(
            "links/special files are not executable: {}",
            path.display()
        ));
    }
    let mut result = Stamp {
        directory: m.is_dir(),
        identity: identity(path, &m)?,
        length: 0,
        modified: String::new(),
        permissions: permissions(&m),
        digest: String::new(),
    };
    if m.is_file() {
        result.length = m.len();
        result.modified = format!("{:?}", io(m.modified())?);
        let mut file = io(File::open(path))?;
        let mut hash = blake3::Hasher::new();
        let mut buf = [0; 65536];
        loop {
            cancel.check()?;
            let n = io(file.read(&mut buf))?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        result.digest = hash.finalize().to_hex().to_string();
        let after = io(file.metadata())?;
        if m.len() != after.len()
            || m.modified().ok() != after.modified().ok()
            || identity(path, &after)? != result.identity
        {
            return Err("file changed while fingerprinting".into());
        }
    }
    Ok(result)
}
pub(crate) fn capture(plan: &Plan, cancel: &Cancellation) -> Result<Inventory> {
    let mut trees = Vec::new();
    for root in [&plan.left, &plan.right]
        .into_iter()
        .chain(plan.base.as_ref())
    {
        if io(fs::canonicalize(root))? != *root {
            return Err("input root moved".into());
        }
        let mut entries = BTreeMap::new();
        entries.insert(PathBuf::new(), stamp(root, cancel)?);
        let mut paths = BTreeMap::new();
        folders::scan(root, Path::new(""), true, &mut paths, cancel)?;
        for path in paths.keys() {
            entries.insert(path.clone(), stamp(&root.join(path), cancel)?);
        }
        trees.push(Tree {
            root: root.clone(),
            entries,
        });
    }
    Ok(trees)
}

/// Parse a bounded report; plans without a complete version-two inventory cannot execute.
pub fn read_plan(path: &Path, cancel: &Cancellation) -> Result<Plan> {
    serde_json::from_slice(&crate::files::read_bounded(path, cancel)?).map_err(|e| e.to_string())
}
pub fn validate(plan: &Plan, cancel: &Cancellation) -> Result<()> {
    if plan.version != 2
        || plan.fingerprints.is_none()
        || plan.summary.review != 0
        || plan.summary.uncompared != 0
    {
        return Err("execution requires a complete version 2 plan without unresolved items".into());
    }
    if plan.summary.operations > 10_000 {
        return Err("execution limit: 10000 operations; split the selection".into());
    }
    let fresh = sync_plan::build(
        &plan.left,
        &plan.right,
        plan.base.as_deref(),
        plan.mode,
        cancel,
    )?;
    if plan.report()? != fresh.report()? {
        return Err("plan is stale or edited; generate and review a new plan".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    index: usize,
    before: Option<Stamp>,
    after: Option<Stamp>,
    reverted: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    plan: Plan,
    steps: Vec<Step>,
    finished: bool,
    restoring: bool,
    plan_hash: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct RunReport {
    pub run_directory: PathBuf,
    pub completed: usize,
    pub rolled_back: bool,
    pub error: Option<String>,
}
impl RunReport {
    pub fn exit_code(&self) -> i32 {
        match self.error.as_deref() {
            None => 0,
            Some("cancelled") => 130,
            Some(_) => 2,
        }
    }
    pub fn json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Event {
    Header(u32),
    Intent(Step),
    Reverted(usize),
    Restoring,
    PlanHash(String),
    Finished,
}
fn record(run: &Path, event: &Event) -> Result<()> {
    let mut bytes = serde_json::to_vec(event).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut file = io(OpenOptions::new()
        .create(true)
        .append(true)
        .open(run.join("journal.jsonl")))?;
    if io(file.metadata())?.len() + bytes.len() as u64 > 32 * 1024 * 1024 {
        return Err("execution journal exceeds 32 MiB".into());
    }
    io(file.write_all(&bytes))?;
    io(file.sync_all())
}
fn read_journal(run: &Path, cancel: &Cancellation) -> Result<(Journal, u64)> {
    let plan = read_plan(&run.join("plan.json"), cancel)?;
    let bytes = crate::files::read_bounded(&run.join("journal.jsonl"), cancel)?;
    let mut journal = Journal {
        version: 0,
        plan,
        steps: Vec::new(),
        finished: false,
        restoring: false,
        plan_hash: None,
    };
    let mut complete = 0;
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        cancel.check()?;
        if !line.ends_with(b"\n") {
            break;
        } // A crash can leave a partial last record.
        let event: Event = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        match event {
            Event::Header(v) if complete == 0 => journal.version = v,
            Event::PlanHash(hash)
                if journal.version == 1
                    && journal.plan_hash.is_none()
                    && journal.steps.is_empty()
                    && !journal.finished
                    && !journal.restoring =>
            {
                journal.plan_hash = Some(hash)
            }
            Event::Restoring if journal.version == 1 && !journal.restoring => {
                journal.restoring = true
            }
            Event::Intent(step)
                if journal.version == 1
                    && !journal.restoring
                    && !step.reverted
                    && !journal.finished
                    && journal.steps.last().is_none_or(|s| s.index < step.index) =>
            {
                journal.steps.push(step)
            }
            Event::Reverted(index) if journal.version == 1 => {
                let step = journal
                    .steps
                    .iter_mut()
                    .find(|s| s.index == index)
                    .ok_or("invalid restoration record")?;
                step.reverted = true;
                journal.restoring = true;
            }
            Event::Finished if journal.version == 1 && !journal.finished && !journal.restoring => {
                journal.finished = true
            }
            _ => return Err("invalid journal record order".into()),
        }
        complete += line.len() as u64;
    }
    Ok((journal, complete))
}
#[cfg(test)]
fn save_journal(run: &Path, journal: &Journal) -> Result<()> {
    io(File::create(run.join("journal.jsonl")))?;
    record(run, &Event::Header(journal.version))?;
    if let Some(hash) = &journal.plan_hash {
        record(run, &Event::PlanHash(hash.clone()))?;
    }
    for step in &journal.steps {
        record(run, &Event::Intent(step.clone()))?;
    }
    if journal.finished {
        record(run, &Event::Finished)?;
    }
    Ok(())
}
// Unlike fs::rename on Unix this never replaces an existing destination.
fn move_new(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let to = CString::new(to.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (from, to);
        Err("execution currently requires Windows or Linux".into())
    }
}
fn root_for<'a>(plan: &'a Plan, target: &Path) -> Result<&'a Path> {
    [&plan.left, &plan.right]
        .into_iter()
        .find(|r| target.starts_with(r) && target != r.as_path())
        .map(|r| r.as_path())
        .ok_or_else(|| "destination is outside input trees".into())
}
fn guard(plan: &Plan, target: &Path, cancel: &Cancellation) -> Result<()> {
    let root = root_for(plan, target)?;
    if io(fs::canonicalize(root))? != root {
        return Err("root moved".into());
    }
    let relative = target.strip_prefix(root).map_err(|e| e.to_string())?;
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("unsafe relative path".into());
    }
    let mut parent = root.to_path_buf();
    if !stamp(&parent, cancel)?.directory {
        return Err("root is not a directory".into());
    }
    for part in relative.parent().unwrap_or(Path::new("")).components() {
        parent.push(part);
        if !stamp(&parent, cancel)?.directory {
            return Err("ancestor is not a directory".into());
        }
    }
    Ok(())
}
fn optional_stamp(path: &Path, cancel: &Cancellation) -> Result<Option<Stamp>> {
    match fs::symlink_metadata(path) {
        Ok(_) => stamp(path, cancel).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn expected<'a>(inventory: &'a Inventory, path: &Path) -> Result<Option<&'a Stamp>> {
    for tree in inventory {
        if let Ok(rel) = path.strip_prefix(&tree.root) {
            return Ok(tree.entries.get(rel));
        }
    }
    Err("path outside inventory".into())
}
fn update(inventory: &mut Inventory, path: &Path, value: Option<Stamp>) -> Result<()> {
    for tree in inventory {
        if let Ok(rel) = path.strip_prefix(&tree.root) {
            if let Some(value) = value {
                tree.entries.insert(rel.to_path_buf(), value);
            } else {
                tree.entries.remove(rel);
            }
            return Ok(());
        }
    }
    Err("path outside inventory".into())
}
fn empty(path: &Path) -> Result<()> {
    if io(fs::read_dir(path))?.next().is_some() {
        Err(format!("directory is no longer empty: {}", path.display()))
    } else {
        Ok(())
    }
}
fn private_path(run: &Path, area: &str, index: usize) -> PathBuf {
    run.join(area).join(index.to_string())
}
fn same_volume(a: &Stamp, b: &Stamp) -> bool {
    a.identity.split(':').next() == b.identity.split(':').next()
}
fn check_run_location(plan: &Plan, run: &Path) -> Result<()> {
    plan.validate_report_path(&run.join("journal.json"))?;
    for root in [&plan.left, &plan.right]
        .into_iter()
        .chain(plan.base.as_ref())
    {
        if root.starts_with(run) {
            return Err("run directory overlaps an input tree".into());
        }
    }
    Ok(())
}

/// Create a fresh run; an existing run can only be continued with `resume`.
/// Errors after creating the run directory return its recovery path in RunReport.
pub fn apply(plan: Plan, run: &Path, cancel: &Cancellation) -> Result<RunReport> {
    validate(&plan, cancel)?;
    let parent = io(fs::canonicalize(
        run.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    ))?;
    let run = parent.join(run.file_name().ok_or("run directory must have a name")?);
    // Validate before creating anything, including when the requested directory is in an input.
    plan.validate_report_path(&run)?;
    for root in [&plan.left, &plan.right]
        .into_iter()
        .chain(plan.base.as_ref())
    {
        if run.starts_with(root) || root.starts_with(&run) {
            return Err("run directory must be outside input trees".into());
        }
    }
    let parent_stamp = stamp(&parent, cancel)?;
    let inventory = plan.fingerprints.as_ref().ok_or("missing inventory")?;
    for tree in inventory.iter().take(2) {
        if !same_volume(
            &parent_stamp,
            tree.entries.get(Path::new("")).ok_or("missing root")?,
        ) {
            return Err("backup directory and both inputs must be on the same filesystem".into());
        }
    }
    io(fs::create_dir(&run))?;
    let mut journal = Journal {
        version: 1,
        plan,
        steps: Vec::new(),
        finished: false,
        restoring: false,
        plan_hash: None,
    };
    let mut count = 0;
    let result = (|| {
        let lock = io(OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(run.join("lock")))?;
        lock.try_lock().map_err(|e| e.to_string())?;
        for name in ["backups", "staging", "undone"] {
            io(fs::create_dir(run.join(name)))?;
        }
        let report = journal.plan.report()?;
        if report.len() > 32 * 1024 * 1024 {
            return Err("saved plan exceeds 32 MiB".into());
        }
        let mut header = io(OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(run.join("plan.json")))?;
        io(header.write_all(report.as_bytes()))?;
        io(header.sync_all())?;
        record(&run, &Event::Header(1))?;
        let hash = blake3::hash(report.as_bytes()).to_hex().to_string();
        record(&run, &Event::PlanHash(hash.clone()))?;
        journal.plan_hash = Some(hash);
        let mut state = journal
            .plan
            .fingerprints
            .clone()
            .ok_or("missing inventory")?;
        if capture(&journal.plan, cancel)? != state {
            return Err("inputs changed before execution".into());
        }
        execute_remaining(&run, &mut journal, &mut state, &mut count, cancel)
    })();
    Ok(RunReport {
        run_directory: run,
        completed: count,
        rolled_back: false,
        error: result.err(),
    })
}

fn plan_hash(plan: &Plan) -> Result<String> {
    Ok(blake3::hash(plan.report()?.as_bytes()).to_hex().to_string())
}
fn archive_staging(run: &Path, index: usize, cancel: &Cancellation) -> Result<()> {
    let staged = private_path(run, "staging", index);
    if optional_stamp(&staged, cancel)?.is_none() {
        return Ok(());
    }
    let abandoned = run.join("abandoned");
    if optional_stamp(&abandoned, cancel)?.is_none() {
        io(fs::create_dir(&abandoned))?;
    }
    if !stamp(&abandoned, cancel)?.directory {
        return Err("invalid abandoned staging directory".into());
    }
    let name = format!(
        "{}-{}",
        index,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    move_new(&staged, &abandoned.join(name))
}
fn execute_remaining(
    run: &Path,
    journal: &mut Journal,
    state: &mut Inventory,
    count: &mut usize,
    cancel: &Cancellation,
) -> Result<()> {
    for index in 0..journal.plan.items.len() {
        cancel.check()?;
        let item = &journal.plan.items[index];
        if !item.action.operation() || journal.steps.last().is_some_and(|step| index <= step.index)
        {
            continue;
        }
        let target = item.destination.as_ref().ok_or("missing destination")?;
        guard(&journal.plan, target, cancel)?;
        for parent in target.ancestors().skip(1) {
            if let Ok(Some(want)) = expected(state, parent)
                && stamp(parent, cancel)? != *want
            {
                return Err("destination ancestor changed".into());
            }
        }
        let before = expected(state, target)?.cloned();
        archive_staging(run, index, cancel)?;
        let staged = private_path(run, "staging", index);
        let after = match item.action {
            Action::CreateDirectory => {
                io(fs::create_dir(&staged))?;
                Some(stamp(&staged, cancel)?)
            }
            Action::CreateFile | Action::ReplaceFile => {
                let source = item.source.as_ref().ok_or("missing source")?;
                guard(&journal.plan, source, cancel)?;
                let source_stamp = expected(state, source)?.ok_or("missing source fingerprint")?;
                let mut src = io(File::open(source))?;
                let mut dst = io(OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&staged))?;
                let mut buf = [0; 65536];
                loop {
                    cancel.check()?;
                    let n = io(src.read(&mut buf))?;
                    if n == 0 {
                        break;
                    }
                    io(dst.write_all(&buf[..n]))?;
                }
                let meta = io(src.metadata())?;
                io(dst.set_times(fs::FileTimes::new().set_modified(io(meta.modified())?)))?;
                io(dst.set_permissions(meta.permissions()))?;
                io(dst.sync_all())?;
                drop(dst);
                drop(src);
                let after = stamp(&staged, cancel)?;
                if after.digest != source_stamp.digest
                    || after.length != source_stamp.length
                    || stamp(source, cancel)? != *source_stamp
                {
                    return Err("source changed while preparing copy".into());
                }
                Some(after)
            }
            _ => None,
        };
        if optional_stamp(target, cancel)? != before {
            return Err("destination changed before execution".into());
        }
        if item.action == Action::RemoveDirectory {
            empty(target)?;
        }
        // Persist intent with candidate identity before any input mutation.
        journal.steps.push(Step {
            index,
            before: before.clone(),
            after: after.clone(),
            reverted: false,
        });
        record(
            run,
            &Event::Intent(journal.steps.last().ok_or("missing intent")?.clone()),
        )?;
        guard(&journal.plan, target, cancel)?;
        if let Some(before) = &before {
            let backup = private_path(run, "backups", index);
            move_new(target, &backup)?;
            if stamp(&backup, cancel)? != *before {
                return Err(
                    "destination changed while moving to backup; original preserved in backups"
                        .into(),
                );
            }
            if before.directory {
                empty(&backup)?;
            }
        }
        if after.as_ref().is_some_and(|s| s.directory) {
            empty(&staged)?;
        }
        if after.is_some() {
            move_new(&staged, target)?;
        }
        update(state, target, after)?;
        *count += 1;
    }
    if capture(&journal.plan, cancel)? != *state {
        return Err("inputs changed during final verification".into());
    }
    journal.finished = true;
    record(run, &Event::Finished)
}

fn validate_journal(run: &Path, journal: &Journal, cancel: &Cancellation) -> Result<()> {
    if journal.version != 1 || journal.plan.version != 2 {
        return Err("unsupported journal version".into());
    }
    check_run_location(&journal.plan, run)?;
    for tree in journal
        .plan
        .fingerprints
        .as_ref()
        .ok_or("missing inventory")?
    {
        if stamp(&tree.root, cancel)?
            != *tree
                .entries
                .get(Path::new(""))
                .ok_or("missing root fingerprint")?
        {
            return Err("input root changed; recovery refused".into());
        }
    }
    for name in ["backups", "staging", "undone"] {
        if !stamp(&run.join(name), cancel)?.directory {
            return Err("invalid backup directory".into());
        }
    }
    // Rebuild paths and actions from the saved inventory-independent plan shape.
    // Validate every stored index/path before making any restoration.
    let mut previous = None;
    for step in &journal.steps {
        cancel.check()?;
        if previous.is_some_and(|p| step.index <= p) {
            return Err("invalid journal order".into());
        }
        previous = Some(step.index);
        let item = journal
            .plan
            .items
            .get(step.index)
            .ok_or("invalid journal index")?;
        let target = item
            .destination
            .as_deref()
            .ok_or("missing journal destination")?;
        let root = root_for(&journal.plan, target)?;
        if target != root.join(&item.path)
            || item
                .path
                .components()
                .any(|p| !matches!(p, Component::Normal(_)))
            || !item.action.operation()
        {
            return Err("unsafe journal destination".into());
        }
        if expected(
            journal
                .plan
                .fingerprints
                .as_ref()
                .ok_or("missing inventory")?,
            target,
        )?
        .cloned()
            != step.before
        {
            return Err("journal backup fingerprint mismatch".into());
        }
    }
    if let Some(hash) = &journal.plan_hash
        && *hash != plan_hash(&journal.plan)?
    {
        return Err("saved execution plan was edited".into());
    }
    Ok(())
}
fn trim_log(run: &Path, complete: u64) -> Result<()> {
    let log = io(OpenOptions::new()
        .write(true)
        .open(run.join("journal.jsonl")))?;
    io(log.set_len(complete))?;
    io(log.sync_all())
}
/// Restore in reverse order, refusing to replace a user-edited result. Created
/// results are moved into `undone`; originals and unexpected files are never deleted.
pub fn rollback(run: &Path, cancel: &Cancellation) -> Result<RunReport> {
    let run = io(fs::canonicalize(run))?;
    let lock = io(OpenOptions::new()
        .read(true)
        .write(true)
        .open(run.join("lock")))?;
    lock.try_lock().map_err(|e| e.to_string())?;
    let (mut journal, complete) = read_journal(&run, cancel)?;
    validate_journal(&run, &journal, cancel)?;
    // Mark restoration durably before any original can be moved back.
    trim_log(&run, complete)?;
    if !journal.restoring {
        record(&run, &Event::Restoring)?;
        journal.restoring = true;
    }
    let mut count = 0;
    let result = (|| {
        for n in (0..journal.steps.len()).rev() {
            cancel.check()?;
            let step = journal.steps[n].clone();
            if step.reverted {
                continue;
            }
            let target = journal.plan.items[step.index]
                .destination
                .as_ref()
                .ok_or("missing destination")?;
            guard(&journal.plan, target, cancel)?;
            let backup = private_path(&run, "backups", step.index);
            let undone = private_path(&run, "undone", step.index);
            let current = optional_stamp(target, cancel)?;
            let saved = optional_stamp(&backup, cancel)?;
            let removed = optional_stamp(&undone, cancel)?;
            if saved.as_ref().is_some_and(|s| s.directory) {
                empty(&backup)?;
            }
            // Nothing happened, or restoration completed before a journal update.
            if current == step.before && saved.is_none() {
                if removed.is_some() && removed != step.after {
                    return Err("unexpected rollback output".into());
                }
            } else {
                if saved != step.before {
                    return Err("backup missing or edited; restoration refused".into());
                }
                if current.is_some() {
                    if current != step.after || removed.is_some() {
                        return Err(format!(
                            "result changed; restoration refused: {}",
                            target.display()
                        ));
                    }
                    if current.as_ref().is_some_and(|s| s.directory) {
                        empty(target)?;
                    }
                    move_new(target, &undone)?;
                    if optional_stamp(&undone, cancel)? != step.after {
                        return Err("result changed during restoration; preserved in undone".into());
                    }
                } else if removed.is_some() && removed != step.after {
                    return Err("unexpected rollback output".into());
                }
                if saved.is_some() {
                    move_new(&backup, target)?;
                }
            }
            journal.steps[n].reverted = true;
            record(&run, &Event::Reverted(step.index))?;
            count += 1;
        }
        Ok(())
    })();
    Ok(RunReport {
        run_directory: run,
        completed: count,
        rolled_back: result.is_ok(),
        error: result.err(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_plan::Mode;
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let run = tmp.path().join("run");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        (tmp, left, right, run)
    }
    fn plan(left: &Path, right: &Path, mode: Mode) -> Plan {
        sync_plan::build(left, right, None, mode, &Cancellation::new()).unwrap()
    }
    #[test]
    fn mirror_round_trip_preserves_originals_and_repeated_rollback_is_safe() {
        let (_tmp, left, right, run) = setup();
        let c = Cancellation::new();
        fs::create_dir(left.join("new")).unwrap();
        fs::write(left.join("new/file"), b"new").unwrap();
        fs::write(left.join("replace"), b"updated").unwrap();
        fs::write(right.join("replace"), b"original").unwrap();
        fs::create_dir(right.join("old")).unwrap();
        fs::write(right.join("old/file"), b"retired").unwrap();
        let p = plan(&left, &right, Mode::MirrorRight);
        let before = p.fingerprints.clone().unwrap();
        let report = apply(p, &run, &c).unwrap();
        assert_eq!(report.error, None);
        assert_eq!(fs::read(right.join("replace")).unwrap(), b"updated");
        assert!(!right.join("old").exists());
        assert_eq!(fs::read(left.join("replace")).unwrap(), b"updated");
        let report = rollback(&run, &c).unwrap();
        assert_eq!(report.error, None);
        assert!(report.rolled_back);
        let restored = plan(&left, &right, Mode::MirrorRight);
        assert_eq!(restored.fingerprints.unwrap(), before);
        assert_eq!(rollback(&run, &c).unwrap().error, None);
    }
    #[test]
    fn stale_tampered_linked_and_overlapping_runs_do_not_write_inputs() {
        let (_tmp, left, right, run) = setup();
        let c = Cancellation::new();
        fs::write(left.join("file"), b"before").unwrap();
        let p = plan(&left, &right, Mode::UpdateRight);
        fs::write(left.join("file"), b"changed").unwrap();
        assert!(apply(p, &run, &c).unwrap_err().contains("stale"));
        assert!(!run.exists());
        let mut p = plan(&left, &right, Mode::UpdateRight);
        p.items[0].destination = Some(left.join("../escape"));
        assert!(apply(p, &run, &c).is_err());
        assert!(!run.exists());
        assert!(
            apply(
                plan(&left, &right, Mode::UpdateRight),
                &right.join("run"),
                &c
            )
            .is_err()
        );
        assert!(!right.join("file").exists());
        let c = Cancellation::new();
        c.cancel();
        assert!(apply(plan(&left, &right, Mode::UpdateRight), &run, &c).is_err());
        assert!(!run.exists());
    }
    #[test]
    fn rollback_refuses_user_edits_and_preserves_backup() {
        let (_tmp, left, right, run) = setup();
        let c = Cancellation::new();
        fs::write(left.join("file"), b"new").unwrap();
        fs::write(right.join("file"), b"old").unwrap();
        assert_eq!(
            apply(plan(&left, &right, Mode::UpdateRight), &run, &c)
                .unwrap()
                .error,
            None
        );
        fs::write(right.join("file"), b"user edit").unwrap();
        assert!(
            rollback(&run, &c)
                .unwrap()
                .error
                .unwrap()
                .contains("changed")
        );
        assert_eq!(fs::read(right.join("file")).unwrap(), b"user edit");
        let backup = io(fs::read_dir(run.join("backups")))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(backup).unwrap(), b"old");
    }
    #[test]
    fn recover_pending_move_and_interrupted_restore() {
        let (_tmp, left, right, run) = setup();
        let c = Cancellation::new();
        fs::write(left.join("file"), b"new").unwrap();
        fs::write(right.join("file"), b"old").unwrap();
        assert_eq!(
            apply(plan(&left, &right, Mode::UpdateRight), &run, &c)
                .unwrap()
                .error,
            None
        );
        let (j, _) = read_journal(&run, &c).unwrap();
        let index = j.steps[0].index;
        // Simulate a crash after moving the produced file away but before restoring its backup.
        move_new(&right.join("file"), &private_path(&run, "undone", index)).unwrap();
        assert_eq!(rollback(&run, &c).unwrap().error, None);
        assert_eq!(fs::read(right.join("file")).unwrap(), b"old");
        // Same intent, but crash before the durable reverted flag.
        let mut j = j;
        j.steps[0].reverted = false;
        save_journal(&run, &j).unwrap();
        assert_eq!(rollback(&run, &c).unwrap().error, None);
    }
    #[test]
    fn merge_updates_both_sides_and_restores_without_touching_ancestor() {
        let (tmp, left, right, run) = setup();
        let c = Cancellation::new();
        let base = tmp.path().join("base");
        fs::create_dir(&base).unwrap();
        for root in [&base, &left, &right] {
            fs::write(root.join("a"), b"a").unwrap();
            fs::write(root.join("b"), b"b").unwrap();
        }
        fs::write(left.join("a"), b"left").unwrap();
        fs::write(right.join("b"), b"right").unwrap();
        let p = sync_plan::build(&left, &right, Some(&base), Mode::Merge, &c).unwrap();
        let initial = p.fingerprints.clone().unwrap();
        assert_eq!(apply(p, &run, &c).unwrap().error, None);
        assert_eq!(fs::read(right.join("a")).unwrap(), b"left");
        assert_eq!(fs::read(left.join("b")).unwrap(), b"right");
        assert_eq!(rollback(&run, &c).unwrap().error, None);
        assert_eq!(
            sync_plan::build(&left, &right, Some(&base), Mode::Merge, &c)
                .unwrap()
                .fingerprints
                .unwrap(),
            initial
        );
    }
    #[cfg(unix)]
    #[test]
    fn links_are_never_executable_and_atomic_move_never_overwrites() {
        use std::os::unix::fs::symlink;
        let (tmp, left, right, run) = setup();
        let c = Cancellation::new();
        symlink(tmp.path(), left.join("link")).unwrap();
        let p = plan(&left, &right, Mode::MirrorRight);
        assert!(apply(p, &run, &c).is_err());
        assert!(!run.exists());
        fs::write(tmp.path().join("a"), b"a").unwrap();
        fs::write(tmp.path().join("b"), b"b").unwrap();
        assert!(move_new(&tmp.path().join("a"), &tmp.path().join("b")).is_err());
        assert_eq!(fs::read(tmp.path().join("b")).unwrap(), b"b");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::sync_plan::Mode;
    #[test]
    fn no_clobber_moves_locking_partial_records_and_pending_intents() {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let run = tmp.path().join("run");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::write(left.join("file"), b"new").unwrap();
        fs::write(right.join("file"), b"old").unwrap();
        assert!(move_new(&left.join("file"), &right.join("file")).is_err());
        assert_eq!(fs::read(right.join("file")).unwrap(), b"old");
        let c = Cancellation::new();
        let p = sync_plan::build(&left, &right, None, Mode::UpdateRight, &c).unwrap();
        assert!(apply(p, &run, &c).unwrap().error.is_none());
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(run.join("lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(rollback(&run, &c).is_err());
        assert!(resume(&run, &c).is_err());
        drop(lock);
        // The original was moved aside, but candidate installation never completed.
        let (journal, _) = read_journal(&run, &c).unwrap();
        let index = journal.steps[0].index;
        move_new(&right.join("file"), &private_path(&run, "staging", index)).unwrap();
        let mut log = OpenOptions::new()
            .append(true)
            .open(run.join("journal.jsonl"))
            .unwrap();
        log.write_all(b"{\"Revert").unwrap();
        drop(log);
        assert!(rollback(&run, &c).unwrap().error.is_none());
        assert_eq!(fs::read(right.join("file")).unwrap(), b"old");
        assert!(rollback(&run, &c).unwrap().error.is_none());
    }
}

/// Continue only after verifying the complete mixed before/after inventory.
/// Existing version-one logs without a plan checksum remain rollback-only.
pub fn resume(run: &Path, cancel: &Cancellation) -> Result<RunReport> {
    let run = io(fs::canonicalize(run))?;
    let lock = io(OpenOptions::new()
        .read(true)
        .write(true)
        .open(run.join("lock")))?;
    lock.try_lock().map_err(|e| e.to_string())?;
    let (mut journal, complete) = read_journal(&run, cancel)?;
    validate_journal(&run, &journal, cancel)?;
    if journal.plan_hash.is_none() {
        return Err("this older run supports rollback only; no execution-plan checksum".into());
    }
    if journal.restoring || journal.steps.iter().any(|s| s.reverted) {
        return Err(
            "restoration has started; continue rollback instead of resuming synchronization".into(),
        );
    }
    empty(&run.join("undone"))?;
    let plan = &journal.plan;
    if plan.summary.review != 0 || plan.summary.uncompared != 0 || plan.summary.operations > 10_000
    {
        return Err("invalid executable plan summary".into());
    }
    let operations: Vec<usize> = plan
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| item.action.operation().then_some(i))
        .collect();
    if operations.len() != plan.summary.operations || journal.steps.len() > operations.len() {
        return Err("invalid operation count".into());
    }
    // Validate paths for not-yet-started operations too, before touching any input.
    for item in plan.items.iter().filter(|i| i.action.operation()) {
        let target = item.destination.as_ref().ok_or("missing destination")?;
        let root = root_for(plan, target)?;
        if item.path.as_os_str().is_empty()
            || target != &root.join(&item.path)
            || item
                .path
                .components()
                .any(|p| !matches!(p, Component::Normal(_)))
        {
            return Err("unsafe planned destination".into());
        }
        if let Some(source) = &item.source {
            let source_root = root_for(plan, source)?;
            if source != &source_root.join(&item.path) || source_root == root {
                return Err("unsafe planned source".into());
            }
        }
    }
    let mut state = plan.fingerprints.clone().ok_or("missing inventory")?;
    let mut pending = None;
    let mut count = 0;
    for (position, step) in journal.steps.iter().enumerate() {
        cancel.check()?;
        if operations[position] != step.index {
            return Err("execution intents must be an operation prefix".into());
        }
        let item = &plan.items[step.index];
        let target = item.destination.as_ref().ok_or("missing destination")?;
        let saved = optional_stamp(&private_path(&run, "backups", step.index), cancel)?;
        let staged = optional_stamp(&private_path(&run, "staging", step.index), cancel)?;
        let current = optional_stamp(target, cancel)?;
        if saved.as_ref().is_some_and(|s| s.directory) {
            empty(&private_path(&run, "backups", step.index))?;
        }
        if staged.as_ref().is_some_and(|s| s.directory) {
            empty(&private_path(&run, "staging", step.index))?;
        }
        // Every prepared candidate has the original source content and metadata.
        match item.action {
            Action::CreateFile | Action::ReplaceFile => {
                let src = expected(&state, item.source.as_ref().ok_or("missing source")?)?
                    .ok_or("missing source fingerprint")?;
                let after = step.after.as_ref().ok_or("missing prepared candidate")?;
                if after.directory
                    || after.digest != src.digest
                    || after.length != src.length
                    || after.modified != src.modified
                    || after.permissions != src.permissions
                {
                    return Err("prepared copy does not match its source".into());
                }
            }
            Action::CreateDirectory
                if step.before.is_none() && step.after.as_ref().is_some_and(|s| s.directory) => {}
            Action::DeleteFile | Action::RemoveDirectory
                if step.before.is_some() && step.after.is_none() => {}
            _ => return Err("invalid prepared operation".into()),
        }
        let done = current == step.after && saved == step.before && staged.is_none();
        let untouched = current == step.before && saved.is_none() && staged == step.after;
        let backed_up = step.before.is_some()
            && saved == step.before
            && current.is_none()
            && staged == step.after;
        if done {
            update(&mut state, target, step.after.clone())?;
            count += 1;
        } else if position + 1 == journal.steps.len()
            && !journal.finished
            && (untouched || backed_up)
        {
            if backed_up {
                update(&mut state, target, None)?;
            }
            pending = Some(step.clone());
        } else {
            return Err(format!(
                "execution state changed; resume refused: {}",
                target.display()
            ));
        }
    }
    if journal.finished && (pending.is_some() || journal.steps.len() != operations.len()) {
        return Err("incomplete finished journal".into());
    }
    if capture(plan, cancel)? != state {
        return Err("inputs changed since interruption; resume refused".into());
    }
    if journal.finished {
        return Ok(RunReport {
            run_directory: run,
            completed: count,
            rolled_back: false,
            error: None,
        });
    }
    trim_log(&run, complete)?;
    let result = (|| {
        if let Some(step) = pending {
            let item = &journal.plan.items[step.index];
            let target = item.destination.as_ref().ok_or("missing destination")?;
            guard(&journal.plan, target, cancel)?;
            if optional_stamp(target, cancel)? != expected(&state, target)?.cloned() {
                return Err("pending destination changed".into());
            }
            let backup = private_path(&run, "backups", step.index);
            if let Some(before) = &step.before {
                if optional_stamp(&backup, cancel)?.is_none() {
                    move_new(target, &backup)?;
                }
                if stamp(&backup, cancel)? != *before {
                    return Err("backup changed; resume stopped".into());
                }
                if before.directory {
                    empty(&backup)?;
                }
            }
            if let Some(after) = &step.after {
                let staged = private_path(&run, "staging", step.index);
                if stamp(&staged, cancel)? != *after {
                    return Err("prepared candidate changed".into());
                }
                if after.directory {
                    empty(&staged)?;
                }
                move_new(&staged, target)?;
            }
            update(&mut state, target, step.after)?;
            count += 1;
        }
        execute_remaining(&run, &mut journal, &mut state, &mut count, cancel)
    })();
    Ok(RunReport {
        run_directory: run,
        completed: count,
        rolled_back: false,
        error: result.err(),
    })
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use crate::sync_plan::Mode;
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let left = tmp.path().join("left");
        let right = tmp.path().join("right");
        let run = tmp.path().join("run");
        fs::create_dir(&left).unwrap();
        fs::create_dir(&right).unwrap();
        fs::create_dir(left.join("new")).unwrap();
        fs::write(left.join("new/file"), b"new").unwrap();
        fs::write(left.join("replace"), b"updated").unwrap();
        fs::write(right.join("replace"), b"original").unwrap();
        fs::create_dir(right.join("old")).unwrap();
        fs::write(right.join("old/file"), b"retired").unwrap();
        fs::create_dir(right.join("empty")).unwrap();
        (tmp, left, right, run)
    }
    fn undo_step(run: &Path, journal: &Journal, n: usize, restore_original: bool) {
        let step = &journal.steps[n];
        let target = journal.plan.items[step.index].destination.as_ref().unwrap();
        if step.after.is_some() {
            move_new(target, &private_path(run, "staging", step.index)).unwrap();
        }
        if restore_original && step.before.is_some() {
            move_new(&private_path(run, "backups", step.index), target).unwrap();
        }
    }
    #[test]
    fn every_operation_boundary_resumes_without_repeating_and_remains_reversible() {
        // New directory, creation, replacement, deletion and deepest-first directory removal.
        for boundary in 0..=6 {
            for phase in 0..3 {
                let (_tmp, left, right, run) = setup();
                let c = Cancellation::new();
                let p = sync_plan::build(&left, &right, None, Mode::MirrorRight, &c).unwrap();
                let original = p.fingerprints.clone().unwrap();
                assert!(apply(p, &run, &c).unwrap().error.is_none());
                let (mut journal, _) = read_journal(&run, &c).unwrap();
                let total = journal.steps.len();
                assert_eq!(total, 6);
                for n in (boundary..total).rev() {
                    undo_step(&run, &journal, n, true);
                }
                if boundary > 0 && phase < 2 {
                    undo_step(&run, &journal, boundary - 1, phase == 0);
                }
                journal.steps.truncate(boundary);
                journal.finished = false;
                save_journal(&run, &journal).unwrap();
                // An incomplete tail is discarded before appending new durable intents.
                OpenOptions::new()
                    .append(true)
                    .open(run.join("journal.jsonl"))
                    .unwrap()
                    .write_all(b"{\"Intent\":")
                    .unwrap();
                let report = resume(&run, &c).unwrap();
                assert!(
                    report.error.is_none(),
                    "boundary {boundary}, phase {phase}: {:?}",
                    report.error
                );
                assert_eq!(report.completed, total);
                assert!(
                    folders::compare(&left, &right, true, &c)
                        .unwrap()
                        .iter()
                        .all(|e| e.status == folders::Status::Equal)
                );
                assert_eq!(resume(&run, &c).unwrap().error, None);
                assert_eq!(rollback(&run, &c).unwrap().error, None);
                assert_eq!(capture(&journal.plan, &c).unwrap(), original);
                assert!(resume(&run, &c).unwrap_err().contains("restoration"));
            }
        }
    }
    #[test]
    fn changed_results_inputs_backups_and_plan_are_rejected_before_more_writes() {
        for what in [
            "result", "source", "extra", "backup", "plan", "legacy", "restore",
        ] {
            let (_tmp, left, right, run) = setup();
            let c = Cancellation::new();
            assert!(
                apply(
                    sync_plan::build(&left, &right, None, Mode::MirrorRight, &c).unwrap(),
                    &run,
                    &c
                )
                .unwrap()
                .error
                .is_none()
            );
            let (mut j, _) = read_journal(&run, &c).unwrap();
            j.finished = false;
            save_journal(&run, &j).unwrap();
            match what {
                "result" => fs::write(right.join("replace"), b"edited").unwrap(),
                "source" => fs::write(left.join("replace"), b"edited").unwrap(),
                "extra" => fs::write(right.join("unexpected"), b"edited").unwrap(),
                "backup" => {
                    let step = j
                        .steps
                        .iter()
                        .find(|s| s.before.as_ref().is_some_and(|s| !s.directory))
                        .unwrap();
                    fs::write(private_path(&run, "backups", step.index), b"edited").unwrap();
                }
                "plan" => {
                    let mut p = j.plan.clone();
                    p.items[0].reason = "edited".into();
                    fs::write(run.join("plan.json"), p.report().unwrap()).unwrap();
                }
                "legacy" => {
                    j.plan_hash = None;
                    save_journal(&run, &j).unwrap();
                }
                "restore" => record(&run, &Event::Restoring).unwrap(),
                _ => unreachable!(),
            }
            let before = capture(&j.plan, &c).unwrap();
            let log = fs::read(run.join("journal.jsonl")).unwrap();
            assert!(resume(&run, &c).is_err(), "{what}");
            assert_eq!(capture(&j.plan, &c).unwrap(), before);
            assert_eq!(fs::read(run.join("journal.jsonl")).unwrap(), log);
        }
    }
    #[test]
    fn directory_contents_added_to_prepared_or_backed_up_directories_are_rejected() {
        for area in ["staging", "backups"] {
            let (_tmp, left, right, run) = setup();
            let c = Cancellation::new();
            assert_eq!(
                apply(
                    sync_plan::build(&left, &right, None, Mode::MirrorRight, &c).unwrap(),
                    &run,
                    &c
                )
                .unwrap()
                .error,
                None
            );
            let (mut j, _) = read_journal(&run, &c).unwrap();
            j.finished = false;
            let index = if area == "staging" {
                for n in (0..j.steps.len()).rev() {
                    undo_step(&run, &j, n, true);
                }
                j.steps.truncate(1);
                j.steps[0].index
            } else {
                j.steps
                    .iter()
                    .find(|s| s.before.as_ref().is_some_and(|s| s.directory))
                    .unwrap()
                    .index
            };
            save_journal(&run, &j).unwrap();
            let extra = private_path(&run, area, index).join("unexpected");
            fs::write(&extra, b"keep me").unwrap();
            let before = capture(&j.plan, &c).unwrap();
            assert!(resume(&run, &c).is_err());
            assert_eq!(capture(&j.plan, &c).unwrap(), before);
            assert_eq!(fs::read(&extra).unwrap(), b"keep me");
            if area == "backups" {
                assert!(rollback(&run, &c).unwrap().error.is_some());
                assert_eq!(fs::read(extra).unwrap(), b"keep me");
            }
        }
    }
    #[test]
    fn changed_prepared_candidate_and_cancelled_request_keep_backups_and_partial_state() {
        let (_tmp, left, right, run) = setup();
        let c = Cancellation::new();
        assert!(
            apply(
                sync_plan::build(&left, &right, None, Mode::MirrorRight, &c).unwrap(),
                &run,
                &c
            )
            .unwrap()
            .error
            .is_none()
        );
        let (mut j, _) = read_journal(&run, &c).unwrap();
        let n = j
            .steps
            .iter()
            .position(|s| s.before.as_ref().is_some_and(|s| !s.directory) && s.after.is_some())
            .unwrap();
        for later in (n + 1..j.steps.len()).rev() {
            undo_step(&run, &j, later, true);
        }
        undo_step(&run, &j, n, false);
        j.steps.truncate(n + 1);
        j.finished = false;
        save_journal(&run, &j).unwrap();
        let cancelled = Cancellation::new();
        cancelled.cancel();
        assert!(resume(&run, &cancelled).is_err());
        let staged = private_path(&run, "staging", j.steps[n].index);
        fs::write(&staged, b"edited candidate").unwrap();
        let before = capture(&j.plan, &c).unwrap();
        assert!(resume(&run, &c).is_err());
        assert_eq!(capture(&j.plan, &c).unwrap(), before);
        assert!(private_path(&run, "backups", j.steps[n].index).exists());
        assert_eq!(rollback(&run, &c).unwrap().error, None);
    }
}

#[cfg(test)]
mod resume_directions {
    use super::*;
    use crate::sync_plan::Mode;
    #[test]
    fn leftward_update_mirror_and_bidirectional_ancestor_merge_resume() {
        for mode in [Mode::UpdateLeft, Mode::MirrorLeft, Mode::Merge] {
            let tmp = tempfile::tempdir().unwrap();
            let [base, left, right] = ["base", "left", "right"].map(|n| tmp.path().join(n));
            let run = tmp.path().join("run");
            let c = Cancellation::new();
            for root in [&base, &left, &right] {
                fs::create_dir(root).unwrap();
                fs::write(root.join("a"), b"original a").unwrap();
                fs::write(root.join("b"), b"original b").unwrap();
            }
            fs::write(left.join("a"), b"left edit").unwrap();
            fs::write(right.join("b"), b"right edit").unwrap();
            let p = sync_plan::build(
                &left,
                &right,
                (mode == Mode::Merge).then_some(base.as_path()),
                mode,
                &c,
            )
            .unwrap();
            let initial = p.fingerprints.clone().unwrap();
            assert_eq!(apply(p, &run, &c).unwrap().error, None);
            let (mut j, _) = read_journal(&run, &c).unwrap();
            assert_eq!(j.steps.len(), 2);
            let last = j.steps.pop().unwrap();
            let target = j.plan.items[last.index].destination.as_ref().unwrap();
            move_new(target, &private_path(&run, "staging", last.index)).unwrap();
            move_new(&private_path(&run, "backups", last.index), target).unwrap();
            j.finished = false;
            save_journal(&run, &j).unwrap();
            assert_eq!(resume(&run, &c).unwrap().error, None);
            assert!(
                folders::compare(&left, &right, true, &c)
                    .unwrap()
                    .iter()
                    .all(|e| e.status == folders::Status::Equal)
            );
            assert_eq!(rollback(&run, &c).unwrap().error, None);
            assert_eq!(capture(&j.plan, &c).unwrap(), initial);
        }
    }
}
