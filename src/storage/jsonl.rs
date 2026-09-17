use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A JSONL line that could not be deserialized and was therefore skipped.
///
/// Records are skipped rather than fatal so that a single unreadable line — a
/// record written by a newer rite, or a truncated write — cannot deny access to
/// every other record in the file. Skips are never silent: readers warn (see
/// [`report_issues`]) and `rite doctor` re-scans and reports the totals.
#[derive(Debug, Clone)]
pub struct SkippedLine {
    /// File the line came from.
    pub path: PathBuf,
    /// 1-based line number, when the read started at the top of the file.
    /// `None` for reads that begin at a mid-file byte offset.
    pub line: Option<u64>,
    /// Byte offset of the first byte of the skipped line.
    pub byte_offset: u64,
    /// Why the line could not be parsed.
    pub error: String,
}

impl fmt::Display for SkippedLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{}: {}", self.path.display(), line, self.error),
            None => write!(
                f,
                "{}@byte {}: {}",
                self.path.display(),
                self.byte_offset,
                self.error
            ),
        }
    }
}

/// A field a deserializer dropped inside a record it otherwise kept.
///
/// Distinct from [`SkippedLine`] on purpose. A skipped line is a record this
/// build lost entirely. A damaged field is a record this build kept, body
/// intact, minus one value it could not read. Both are data loss and both are
/// reported, but conflating them would hide which one happened — and they call
/// for different responses.
///
/// The only producer today is `Message::reply_to`: a reply anchor it cannot
/// read is dropped so the message itself survives, which silently demotes a
/// reply to a top-level message. That is cheap to live with and expensive to
/// not know about, since a lost anchor is exactly what makes an
/// acknowledgment fail to correlate.
#[derive(Debug, Clone)]
pub struct DamagedField {
    /// File the record came from.
    pub path: PathBuf,
    /// 1-based line number, when the read started at the top of the file.
    pub line: Option<u64>,
    /// Byte offset of the first byte of the record's line.
    pub byte_offset: u64,
    /// Name of the field that was dropped, e.g. `reply_to`.
    pub field: &'static str,
    /// The value that could not be read, rendered for a human.
    pub value: String,
}

impl fmt::Display for DamagedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let position = match self.line {
            Some(line) => format!("{}:{}", self.path.display(), line),
            None => format!("{}@byte {}", self.path.display(), self.byte_offset),
        };
        write!(
            f,
            "{}: unreadable `{}` = {}",
            position, self.field, self.value
        )
    }
}

/// Everything a whole-file read found wrong but survived.
#[derive(Debug, Clone, Default)]
pub struct ScanIssues {
    /// Records this build could not read at all.
    pub skipped: Vec<SkippedLine>,
    /// Fields dropped from records this build did read.
    pub damaged: Vec<DamagedField>,
}

impl ScanIssues {
    /// True when the file read cleanly.
    pub fn is_empty(&self) -> bool {
        self.skipped.is_empty() && self.damaged.is_empty()
    }

    /// Absorb another file's issues.
    pub fn extend(&mut self, other: ScanIssues) {
        self.skipped.extend(other.skipped);
        self.damaged.extend(other.damaged);
    }
}

thread_local! {
    /// Fields the deserializer currently running chose to drop.
    ///
    /// A `Deserialize` impl sees only the value in front of it: no path, no
    /// line, no byte offset. It reports here, and [`parse_line`] — which knows
    /// all three — drains the channel and attaches the context.
    static DAMAGED_FIELDS: std::cell::RefCell<Vec<(&'static str, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Report a field dropped rather than failing the whole record.
///
/// Call this from a `Deserialize` impl that degrades instead of erroring. The
/// value is picked up by the enclosing read and surfaced through the same
/// path as an unreadable line: stderr once per file, and `rite doctor`.
///
/// Silent degradation is not an option available to this codebase. If a
/// deserializer cannot report, it must fail the record instead.
pub fn report_damaged_field(field: &'static str, value: impl Into<String>) {
    let value = value.into();
    DAMAGED_FIELDS.with(|cell| {
        if let Ok(mut fields) = cell.try_borrow_mut() {
            fields.push((field, value));
        }
    });
}

/// Drain whatever the last deserialize reported.
fn take_damaged_fields() -> Vec<(&'static str, String)> {
    DAMAGED_FIELDS.with(|cell| match cell.try_borrow_mut() {
        Ok(mut fields) => std::mem::take(&mut *fields),
        Err(_) => Vec::new(),
    })
}

/// Files already warned about in this process, keyed by the kind of problem, so
/// a repeated read (a TUI refresh loop, say) does not spam stderr with the same
/// diagnostic — and so a file with both problems still reports both.
fn warned_paths() -> &'static Mutex<HashSet<(PathBuf, &'static str)>> {
    static WARNED: OnceLock<Mutex<HashSet<(PathBuf, &'static str)>>> = OnceLock::new();
    WARNED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether this is the first time this process has warned about `path` for
/// `kind`.
fn first_warning_for(path: &Path, kind: &'static str) -> bool {
    warned_paths()
        .lock()
        .map(|mut seen| seen.insert((path.to_path_buf(), kind)))
        .unwrap_or(false)
}

/// Make lost data observable.
///
/// Every problem is emitted as a `tracing` warning (structured, picked up by
/// telemetry when configured). Additionally, the first occurrence per file per
/// process prints a human-visible note to stderr, because tracing is a no-op
/// unless telemetry is enabled and silent data loss is worse than noise.
fn report_issues(issues: &ScanIssues) {
    for skip in &issues.skipped {
        tracing::warn!(
            path = %skip.path.display(),
            line = skip.line,
            byte_offset = skip.byte_offset,
            error = %skip.error,
            "skipping unreadable JSONL record"
        );
    }

    if let Some(first) = issues.skipped.first()
        && first_warning_for(&first.path, "skipped")
    {
        eprintln!(
            "warning: skipped {} unreadable line(s) in {} (first: {}); run `rite doctor` for details",
            issues.skipped.len(),
            first.path.display(),
            first
        );
    }

    for damaged in &issues.damaged {
        tracing::warn!(
            path = %damaged.path.display(),
            line = damaged.line,
            byte_offset = damaged.byte_offset,
            field = damaged.field,
            value = %damaged.value,
            "dropping unreadable field from an otherwise readable JSONL record"
        );
    }

    if let Some(first) = issues.damaged.first()
        && first_warning_for(&first.path, "damaged-field")
    {
        eprintln!(
            "warning: dropped {} unreadable field value(s) in {} (first: {}); run `rite doctor` for details",
            issues.damaged.len(),
            first.path.display(),
            first
        );
    }
}

/// Try to deserialize one raw JSONL line.
///
/// Returns `None` for blank lines. A line that cannot be parsed is recorded in
/// `issues.skipped` (with file/line/offset context) and yields `None` too, so
/// callers continue with the rest of the file. A line that parses but drops a
/// field is returned *and* recorded in `issues.damaged`.
fn parse_line<T: DeserializeOwned>(
    raw: &[u8],
    path: &Path,
    line: Option<u64>,
    byte_offset: u64,
    issues: &mut ScanIssues,
) -> Option<T> {
    let text = match std::str::from_utf8(raw) {
        Ok(text) => text,
        Err(error) => {
            issues.skipped.push(SkippedLine {
                path: path.to_path_buf(),
                line,
                byte_offset,
                error: format!("invalid UTF-8: {}", error),
            });
            return None;
        }
    };

    if text.trim().is_empty() {
        return None;
    }

    // Discard anything a deserialize outside this reader left behind, so what
    // the next drain returns belongs to this line and no other.
    let _ = take_damaged_fields();

    let parsed = serde_json::from_str(text);

    for (field, value) in take_damaged_fields() {
        issues.damaged.push(DamagedField {
            path: path.to_path_buf(),
            line,
            byte_offset,
            field,
            value,
        });
    }

    match parsed {
        Ok(record) => Some(record),
        Err(error) => {
            issues.skipped.push(SkippedLine {
                path: path.to_path_buf(),
                line,
                byte_offset,
                error: error.to_string(),
            });
            None
        }
    }
}

/// Append a single record to a JSONL file with exclusive locking.
///
/// This ensures safe concurrent writes from multiple processes.
pub fn append_record<T: Serialize>(path: &Path, record: &T) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open file for append: {}", path.display()))?;

    // Acquire exclusive lock (blocks until available)
    file.lock_exclusive()
        .with_context(|| format!("Failed to acquire lock on: {}", path.display()))?;

    // Serialize and write the record
    let json = serde_json::to_string(record).with_context(|| "Failed to serialize record")?;

    let mut writer = std::io::BufWriter::new(&file);
    writeln!(writer, "{}", json)
        .with_context(|| format!("Failed to write to: {}", path.display()))?;

    writer.flush()?;

    // Ensure data is written to disk
    file.sync_all()
        .with_context(|| format!("Failed to sync: {}", path.display()))?;

    // Lock is released when file is dropped
    Ok(())
}

/// Append multiple records to a JSONL file with exclusive locking.
///
/// More efficient than calling `append_record` multiple times.
pub fn append_records<T: Serialize>(path: &Path, records: &[T]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open file for append: {}", path.display()))?;

    file.lock_exclusive()
        .with_context(|| format!("Failed to acquire lock on: {}", path.display()))?;

    let mut writer = std::io::BufWriter::new(&file);

    for record in records {
        let json = serde_json::to_string(record).with_context(|| "Failed to serialize record")?;
        writeln!(writer, "{}", json)
            .with_context(|| format!("Failed to write to: {}", path.display()))?;
    }

    writer.flush()?;
    file.sync_all()?;

    Ok(())
}

/// Atomically check a condition and append a record if the condition is met.
///
/// This function:
/// 1. Acquires an exclusive lock on the file
/// 2. Reads all existing records
/// 3. Calls the predicate function with the records
/// 4. If the predicate returns true, appends the new record
/// 5. Returns whether the append happened
///
/// This is useful for implementing compare-and-swap style operations.
pub fn append_if<T, F>(path: &Path, record: &T, predicate: F) -> Result<bool>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce(&[T]) -> bool,
{
    append_if_reporting(path, record, |records, _| predicate(records))
}

/// Hold the file's exclusive lock while `f` runs, handing it the locked
/// snapshot and what could not be read from it.
///
/// This is a fence, not a write: every writer that goes through
/// [`append_if`] on `path` blocks until `f` returns, so a decision `f` makes
/// about `path` stays true for anything `f` does before returning, including
/// an append to a *different* file. Nothing inside `f` may append to `path`
/// itself; that would wait on this same lock. Callers keep lock order
/// consistent (this file first, then any other) to stay deadlock-free.
pub fn with_exclusive_read<T, R, F>(path: &Path, f: F) -> Result<R>
where
    T: DeserializeOwned,
    F: FnOnce(&[T], &ScanIssues) -> Result<R>,
{
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("Failed to acquire lock on: {}", path.display()))?;
    let (records, issues) = read_locked::<T>(&file, path)?;
    report_issues(&issues);
    f(&records, &issues)
    // Lock is released when `file` is dropped.
}

/// Read every line of an already-locked file.
fn read_locked<T: DeserializeOwned>(
    file: &std::fs::File,
    path: &Path,
) -> Result<(Vec<T>, ScanIssues)> {
    let mut reader = BufReader::new(file);
    let mut records: Vec<T> = Vec::new();
    let mut issues = ScanIssues::default();
    let mut raw = Vec::new();
    let mut byte_offset = 0u64;
    let mut line_no = 0u64;
    loop {
        raw.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut raw)
            .with_context(|| format!("Failed to read from: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        let line_start = byte_offset;
        byte_offset += bytes_read as u64;
        line_no += 1;
        if let Some(rec) = parse_line(&raw, path, Some(line_no), line_start, &mut issues) {
            records.push(rec);
        }
    }
    Ok((records, issues))
}

/// Like [`append_if`], but the predicate also receives what could not be
/// read from the locked snapshot. A caller whose decision must fail closed
/// on a torn or damaged record inspects `issues` under the same lock the
/// append uses, instead of a separate read that a concurrent writer can
/// invalidate.
pub fn append_if_reporting<T, F>(path: &Path, record: &T, predicate: F) -> Result<bool>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce(&[T], &ScanIssues) -> bool,
{
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;

    // Acquire exclusive lock for atomic read-check-write
    file.lock_exclusive()
        .with_context(|| format!("Failed to acquire lock on: {}", path.display()))?;

    // Read existing records while holding the lock. Unreadable lines are
    // skipped (and reported) rather than aborting the check-and-append.
    let mut reader = BufReader::new(&file);
    let mut records: Vec<T> = Vec::new();
    let mut issues = ScanIssues::default();

    let mut raw = Vec::new();
    let mut byte_offset = 0u64;
    let mut line_no = 0u64;

    loop {
        raw.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut raw)
            .with_context(|| format!("Failed to read from: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        let line_start = byte_offset;
        byte_offset += bytes_read as u64;
        line_no += 1;

        if let Some(rec) = parse_line(&raw, path, Some(line_no), line_start, &mut issues) {
            records.push(rec);
        }
    }

    report_issues(&issues);

    // Check if we should append
    if !predicate(&records, &issues) {
        // Lock is released when file is dropped
        return Ok(false);
    }

    // Append the record
    let json = serde_json::to_string(record).with_context(|| "Failed to serialize record")?;

    let mut writer = std::io::BufWriter::new(&file);
    writeln!(writer, "{}", json)
        .with_context(|| format!("Failed to write to: {}", path.display()))?;

    writer.flush()?;
    file.sync_all()?;

    Ok(true)
}

/// Read all records from a JSONL file.
///
/// Returns an empty Vec if the file doesn't exist.
///
/// Lines that cannot be deserialized (records from a newer rite, truncated
/// writes, non-UTF-8 bytes) are skipped and reported rather than failing the
/// whole read — one bad line must not deny access to the rest of the file.
pub fn read_records<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let (records, issues) = read_records_reporting(path)?;
    report_issues(&issues);
    Ok(records)
}

/// Like [`read_records`], but returns what went wrong instead of reporting it,
/// so callers (notably `rite doctor`) can surface the details.
pub fn read_records_reporting<T: DeserializeOwned>(path: &Path) -> Result<(Vec<T>, ScanIssues)> {
    let mut records = Vec::new();
    let issues = scan_records(path, Some(&mut records))?;
    Ok((records, issues))
}

/// Parse every line of a JSONL file as `T` and return only the lines that
/// failed. Nothing is retained, so this is cheap enough to run over every file
/// in the data directory (used by `rite doctor`).
pub fn scan_skipped<T: DeserializeOwned>(path: &Path) -> Result<Vec<SkippedLine>> {
    Ok(scan_issues::<T>(path)?.skipped)
}

/// Parse every line of a JSONL file as `T` and return everything that went
/// wrong: lines lost, and fields dropped from lines that were kept. Nothing is
/// retained, so this is cheap enough to run over the whole data directory.
pub fn scan_issues<T: DeserializeOwned>(path: &Path) -> Result<ScanIssues> {
    scan_records::<T>(path, None)
}

/// Shared implementation of the whole-file read.
///
/// When `records` is `Some`, successfully parsed records are collected into it;
/// when `None` they are parsed and dropped. Either way, problems come back with
/// their file/line/offset context.
fn scan_records<T: DeserializeOwned>(
    path: &Path,
    mut records: Option<&mut Vec<T>>,
) -> Result<ScanIssues> {
    let mut issues = ScanIssues::default();

    if !path.exists() {
        return Ok(issues);
    }

    let file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    // Use shared lock for reading
    file.lock_shared()
        .with_context(|| format!("Failed to acquire shared lock on: {}", path.display()))?;

    let mut reader = BufReader::new(&file);
    let mut raw = Vec::new();
    let mut byte_offset = 0u64;
    let mut line_no = 0u64;

    loop {
        raw.clear();
        let bytes_read = reader.read_until(b'\n', &mut raw).with_context(|| {
            format!(
                "Failed to read line {} from: {}",
                line_no + 1,
                path.display()
            )
        })?;
        if bytes_read == 0 {
            break;
        }

        let line_start = byte_offset;
        byte_offset += bytes_read as u64;
        line_no += 1;

        if let Some(record) = parse_line::<T>(&raw, path, Some(line_no), line_start, &mut issues)
            && let Some(records) = records.as_mut()
        {
            records.push(record);
        }
    }

    Ok(issues)
}

/// Read records from a JSONL file starting at a byte offset.
///
/// Returns the records and the new byte offset after reading.
/// Useful for incremental reading (e.g., tailing a file).
pub fn read_records_from_offset<T: DeserializeOwned>(
    path: &Path,
    offset: u64,
) -> Result<(Vec<T>, u64)> {
    read_records_from_offset_limited(path, offset, None)
}

/// Read up to `limit` records from a JSONL file starting at a byte offset.
///
/// Returns the records and the byte offset immediately after the last line
/// consumed. If the limit is reached before EOF, the returned offset can be
/// used to continue without skipping unread records.
pub fn read_records_from_offset_limited<T: DeserializeOwned>(
    path: &Path,
    offset: u64,
    limit: Option<usize>,
) -> Result<(Vec<T>, u64)> {
    let read = read_from_offset_inner::<T>(path, offset, limit)?;
    report_issues(&read.issues);
    Ok((read.records, read.new_offset))
}

/// What an offset read found, with nothing reported on the caller's behalf.
struct OffsetRead<T> {
    records: Vec<T>,
    new_offset: u64,
    issues: ScanIssues,
}

/// Where a reader that follows a file left off, and proof of what it read
/// to get there: the file's identity and a running SHA-256 over every byte
/// in `[0, offset)`. [`read_records_continuing`] resumes from it only when
/// the same file still has that exact prefix.
#[derive(Clone)]
pub struct Continuation {
    /// Byte offset after the last terminated line consumed. Never inside a
    /// line.
    pub offset: u64,
    /// `(dev, ino)` of the file the offset belongs to.
    pub identity: (u64, u64),
    digest: Sha256,
}

impl fmt::Debug for Continuation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Continuation")
            .field("offset", &self.offset)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// What one locked pass of [`read_records_continuing`] found.
pub struct ContinuedRead<T> {
    /// Records parsed from terminated lines, in file order.
    pub records: Vec<T>,
    /// Whether parsing resumed from the continuation handed in. `false`
    /// means the file was read from its start: no continuation, a different
    /// file, a shorter one, or a prefix that no longer hashes to what was
    /// consumed.
    pub resumed: bool,
    /// Where the next pass continues from, covering exactly the bytes that
    /// were parsed in this and earlier resumed passes.
    pub position: Continuation,
    /// Terminated lines that could not be parsed, with their byte offsets.
    pub issues: ScanIssues,
    /// Byte offset of a final line with no terminating newline: a writer
    /// mid-append, or a file copied in while incomplete. It was neither
    /// parsed nor hashed, and `position` stops before it.
    pub torn_tail: Option<u64>,
}

/// Read `path` under one shared lock as a follower: verify `from` against
/// the file as it is now, parse every terminated line after it (or after
/// the start, if it does not verify), and hash the bytes parsed, so that the
/// returned position describes exactly the bytes whose records were
/// returned and nothing observed at another moment.
///
/// The verification reads the whole prefix, so a pass costs the size of the
/// file, not of the increment. A follower that must never step over a
/// rewrite pays that; one that only wants the increment uses
/// [`read_records_from_offset`].
pub fn read_records_continuing<T: DeserializeOwned>(
    path: &Path,
    from: Option<&Continuation>,
) -> Result<ContinuedRead<T>> {
    let file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;
    file.lock_shared()
        .with_context(|| format!("Failed to acquire shared lock on: {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("Failed to stat: {}", path.display()))?;
    let identity = (meta.dev(), meta.ino());

    let mut reader = BufReader::new(&file);
    let mut resumed = false;
    let mut digest = Sha256::new();
    let mut offset = 0u64;
    if let Some(from) = from
        && from.identity == identity
        && meta.len() >= from.offset
    {
        let mut prefix = Sha256::new();
        let mut remaining = from.offset;
        let mut buf = [0u8; 64 * 1024];
        let mut complete = true;
        while remaining > 0 {
            let want = (buf.len() as u64).min(remaining) as usize;
            let n = reader
                .read(&mut buf[..want])
                .with_context(|| format!("Failed to read from: {}", path.display()))?;
            if n == 0 {
                complete = false;
                break;
            }
            prefix.update(&buf[..n]);
            remaining -= n as u64;
        }
        let expected: [u8; 32] = from.digest.clone().finalize().into();
        let actual: [u8; 32] = prefix.finalize().into();
        if complete && actual == expected {
            resumed = true;
            digest = from.digest.clone();
            offset = from.offset;
        }
    }
    if !resumed {
        reader
            .seek(SeekFrom::Start(0))
            .with_context(|| format!("Failed to seek in: {}", path.display()))?;
    }

    let mut records = Vec::new();
    let mut issues = ScanIssues::default();
    let mut raw = Vec::new();
    let mut torn_tail = None;
    loop {
        raw.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut raw)
            .with_context(|| format!("Failed to read from: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        if raw.last() != Some(&b'\n') {
            torn_tail = Some(offset);
            break;
        }
        let line_start = offset;
        offset += bytes_read as u64;
        digest.update(&raw);
        if let Some(record) = parse_line::<T>(&raw, path, None, line_start, &mut issues) {
            records.push(record);
        }
    }

    Ok(ContinuedRead {
        records,
        resumed,
        position: Continuation {
            offset,
            identity,
            digest,
        },
        issues,
        torn_tail,
    })
}

fn read_from_offset_inner<T: DeserializeOwned>(
    path: &Path,
    offset: u64,
    limit: Option<usize>,
) -> Result<OffsetRead<T>> {
    let empty = |new_offset| OffsetRead {
        records: Vec::new(),
        new_offset,
        issues: ScanIssues::default(),
    };
    if !path.exists() {
        return Ok(empty(0));
    }

    if limit == Some(0) {
        return Ok(empty(offset));
    }

    let mut file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    file.lock_shared()
        .with_context(|| format!("Failed to acquire shared lock on: {}", path.display()))?;

    // Seek to offset
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("Failed to seek in: {}", path.display()))?;

    let mut reader = BufReader::new(&file);
    let mut records = Vec::new();
    let mut issues = ScanIssues::default();
    let mut new_offset = offset;
    let mut raw = Vec::new();

    loop {
        raw.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut raw)
            .with_context(|| format!("Failed to read from: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        let line_start = new_offset;
        new_offset += bytes_read as u64;

        // Absolute line numbers are unknown when starting mid-file, so the
        // byte offset carries the position context here.
        if let Some(record) = parse_line::<T>(&raw, path, None, line_start, &mut issues) {
            records.push(record);

            if limit.is_some_and(|limit| records.len() >= limit) {
                break;
            }
        }
    }

    if limit.is_none() {
        // Get the new offset while still holding the shared lock. Reopening
        // after reading would leave a race where a concurrent append could be
        // included in the returned offset without its records being returned.
        new_offset = reader.seek(SeekFrom::End(0))?;
    }

    Ok(OffsetRead {
        records,
        new_offset,
        issues,
    })
}

/// Count the number of records in a JSONL file.
pub fn count_records(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Ok(0);
    }

    let file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    file.lock_shared()?;

    let reader = BufReader::new(&file);
    let count = reader
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .count();

    Ok(count)
}

/// Read the last N records from a JSONL file.
///
/// This reads the entire file but only returns the last N records.
/// For very large files, consider using offset-based reading instead.
pub fn read_last_n<T: DeserializeOwned>(path: &Path, n: usize) -> Result<Vec<T>> {
    let all_records: Vec<T> = read_records(path)?;
    let start = all_records.len().saturating_sub(n);
    Ok(all_records.into_iter().skip(start).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestRecord {
        id: u32,
        name: String,
    }

    #[test]
    fn test_append_and_read() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        let record1 = TestRecord {
            id: 1,
            name: "Alice".to_string(),
        };
        let record2 = TestRecord {
            id: 2,
            name: "Bob".to_string(),
        };

        append_record(&path, &record1).unwrap();
        append_record(&path, &record2).unwrap();

        let records: Vec<TestRecord> = read_records(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], record1);
        assert_eq!(records[1], record2);
    }

    #[test]
    fn test_append_records_batch() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        let records = vec![
            TestRecord {
                id: 1,
                name: "One".to_string(),
            },
            TestRecord {
                id: 2,
                name: "Two".to_string(),
            },
            TestRecord {
                id: 3,
                name: "Three".to_string(),
            },
        ];

        append_records(&path, &records).unwrap();

        let read: Vec<TestRecord> = read_records(&path).unwrap();
        assert_eq!(read, records);
    }

    #[test]
    fn test_read_nonexistent_file() {
        let path = Path::new("/nonexistent/path/file.jsonl");
        let records: Vec<TestRecord> = read_records(path).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn test_read_from_offset() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        let record1 = TestRecord {
            id: 1,
            name: "First".to_string(),
        };
        append_record(&path, &record1).unwrap();

        // Get current offset
        let (_, offset) = read_records_from_offset::<TestRecord>(&path, 0).unwrap();

        // Add more records
        let record2 = TestRecord {
            id: 2,
            name: "Second".to_string(),
        };
        let record3 = TestRecord {
            id: 3,
            name: "Third".to_string(),
        };
        append_record(&path, &record2).unwrap();
        append_record(&path, &record3).unwrap();

        // Read from offset - should only get new records
        let (new_records, _) = read_records_from_offset::<TestRecord>(&path, offset).unwrap();
        assert_eq!(new_records.len(), 2);
        assert_eq!(new_records[0], record2);
        assert_eq!(new_records[1], record3);
    }

    #[test]
    fn test_read_from_offset_limited_returns_continuation_offset() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        let records: Vec<TestRecord> = (1..=3)
            .map(|id| TestRecord {
                id,
                name: format!("Record{}", id),
            })
            .collect();
        append_records(&path, &records).unwrap();

        let (first, next_offset) =
            read_records_from_offset_limited::<TestRecord>(&path, 0, Some(1)).unwrap();
        assert_eq!(first, vec![records[0].clone()]);
        assert!(next_offset > 0);
        assert!(next_offset < std::fs::metadata(&path).unwrap().len());

        let (remaining, final_offset) =
            read_records_from_offset_limited::<TestRecord>(&path, next_offset, None).unwrap();
        assert_eq!(remaining, records[1..].to_vec());
        assert_eq!(final_offset, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn test_count_records() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        assert_eq!(count_records(&path).unwrap(), 0);

        let records = vec![
            TestRecord {
                id: 1,
                name: "One".to_string(),
            },
            TestRecord {
                id: 2,
                name: "Two".to_string(),
            },
        ];
        append_records(&path, &records).unwrap();

        assert_eq!(count_records(&path).unwrap(), 2);
    }

    /// Write records with an unreadable line wedged in the middle.
    fn file_with_bad_line(path: &Path) {
        use std::io::Write;

        append_record(
            path,
            &TestRecord {
                id: 1,
                name: "first".to_string(),
            },
        )
        .unwrap();

        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        // Shape of a record a newer rite might write: right type name, wrong shape.
        writeln!(file, r#"{{"id":"not-a-number","name":"future"}}"#).unwrap();
        drop(file);

        append_record(
            path,
            &TestRecord {
                id: 3,
                name: "third".to_string(),
            },
        )
        .unwrap();
    }

    #[test]
    fn test_read_records_skips_unparsable_line() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        file_with_bad_line(&path);

        let records: Vec<TestRecord> = read_records(&path).unwrap();
        assert_eq!(records.len(), 2, "one bad line must not lose the good ones");
        assert_eq!(records[0].id, 1);
        assert_eq!(records[1].id, 3);
    }

    /// A record type that keeps going when a field is unreadable, the way
    /// `Message::reply_to` does.
    #[derive(Debug, Serialize, serde::Deserialize)]
    struct LenientRecord {
        id: u64,
        #[serde(default, deserialize_with = "lenient_tag")]
        tag: Option<u64>,
    }

    fn lenient_tag<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<u64>, D::Error> {
        use serde::Deserialize as _;
        let raw = serde_json::Value::deserialize(deserializer)?;
        match raw.as_u64() {
            Some(value) => Ok(Some(value)),
            None => {
                report_damaged_field("tag", raw.to_string());
                Ok(None)
            }
        }
    }

    /// A field a deserializer chose to drop must come back with the file, the
    /// line, and the offending value attached — the same context a skipped
    /// line gets, because it is the same kind of loss.
    #[test]
    fn test_damaged_fields_are_counted_with_context() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("lenient.jsonl");

        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(file, r#"{{"id":1,"tag":7}}"#).unwrap();
            writeln!(file, r#"{{"id":2,"tag":"not-a-number"}}"#).unwrap();
            writeln!(file, r#"{{"id":3}}"#).unwrap();
        }

        let (records, issues) = read_records_reporting::<LenientRecord>(&path).unwrap();

        assert_eq!(records.len(), 3, "no record may be lost to a bad field");
        assert_eq!(records[1].tag, None);
        assert!(issues.skipped.is_empty());

        assert_eq!(issues.damaged.len(), 1);
        let damaged = &issues.damaged[0];
        assert_eq!(damaged.field, "tag");
        assert_eq!(damaged.line, Some(2));
        assert_eq!(damaged.path, path);
        assert!(damaged.value.contains("not-a-number"));
        assert!(damaged.to_string().contains("lenient.jsonl:2"));
        assert!(!issues.is_empty());
    }

    /// Damage reported outside a read must not be misattributed to the next
    /// line a reader happens to parse.
    #[test]
    fn test_stray_damage_is_not_attributed_to_an_unrelated_line() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("clean.jsonl");

        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(file, r#"{{"id":1,"tag":7}}"#).unwrap();
        }

        // Something outside the reader parsed a damaged value — a bare
        // `serde_json::from_str`, say — and left it in the channel.
        report_damaged_field("tag", "stray");

        let (records, issues) = read_records_reporting::<LenientRecord>(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            issues.damaged.is_empty(),
            "the clean line must not inherit someone else's damage: {:?}",
            issues.damaged
        );
    }

    /// Stderr is deduped per file per kind, so a refresh loop cannot spam —
    /// but a file with both problems still reports both.
    #[test]
    fn test_stderr_warnings_dedupe_per_file_and_kind() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("both.jsonl");

        assert!(first_warning_for(&path, "skipped"));
        assert!(!first_warning_for(&path, "skipped"));
        assert!(
            first_warning_for(&path, "damaged-field"),
            "a different kind of loss in the same file still gets one note"
        );
        assert!(!first_warning_for(&path, "damaged-field"));

        let other = temp.path().join("other.jsonl");
        assert!(first_warning_for(&other, "skipped"));
    }

    #[test]
    fn test_read_records_reporting_keeps_line_context() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        file_with_bad_line(&path);

        let (records, issues) = read_records_reporting::<TestRecord>(&path).unwrap();
        let skipped = &issues.skipped;
        assert_eq!(records.len(), 2);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].path, path);
        assert_eq!(skipped[0].line, Some(2));
        assert!(skipped[0].byte_offset > 0);
        assert!(!skipped[0].error.is_empty());
        // Display carries both the file and the position.
        let rendered = skipped[0].to_string();
        assert!(rendered.contains(&path.display().to_string()));
        assert!(rendered.contains(":2:"));
    }

    #[test]
    fn test_scan_skipped_counts_without_collecting() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        file_with_bad_line(&path);

        let skipped = scan_skipped::<TestRecord>(&path).unwrap();
        assert_eq!(skipped.len(), 1);

        // A clean file reports nothing, and a missing file is not an error.
        let clean = temp.path().join("clean.jsonl");
        append_record(
            &clean,
            &TestRecord {
                id: 1,
                name: "ok".to_string(),
            },
        )
        .unwrap();
        assert!(scan_skipped::<TestRecord>(&clean).unwrap().is_empty());
        assert!(
            scan_skipped::<TestRecord>(&temp.path().join("missing.jsonl"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_read_from_offset_skips_unparsable_line() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        file_with_bad_line(&path);

        let (records, offset) = read_records_from_offset::<TestRecord>(&path, 0).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(offset, std::fs::metadata(&path).unwrap().len());

        // Limits count parsed records, and the cursor stays usable across a skip.
        let (first, next) =
            read_records_from_offset_limited::<TestRecord>(&path, 0, Some(1)).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id, 1);
        let (rest, _) =
            read_records_from_offset_limited::<TestRecord>(&path, next, Some(1)).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, 3, "the skipped line must not stall the cursor");
    }

    #[test]
    fn test_append_if_tolerates_unparsable_line() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        file_with_bad_line(&path);

        let new = TestRecord {
            id: 4,
            name: "fourth".to_string(),
        };
        let appended =
            append_if(&path, &new, |existing: &[TestRecord]| existing.len() == 2).unwrap();
        assert!(appended);

        let records: Vec<TestRecord> = read_records(&path).unwrap();
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn test_non_utf8_line_is_skipped() {
        use std::io::Write;

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        {
            let mut file = File::create(&path).unwrap();
            file.write_all(&[0xff, 0xfe, b'\n']).unwrap();
        }
        append_record(
            &path,
            &TestRecord {
                id: 7,
                name: "after".to_string(),
            },
        )
        .unwrap();

        let (records, issues) = read_records_reporting::<TestRecord>(&path).unwrap();
        let skipped = &issues.skipped;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 7);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].error.contains("UTF-8"));
    }

    #[test]
    fn test_read_last_n() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");

        let records: Vec<TestRecord> = (1..=10)
            .map(|i| TestRecord {
                id: i,
                name: format!("Record{}", i),
            })
            .collect();

        append_records(&path, &records).unwrap();

        let last3: Vec<TestRecord> = read_last_n(&path, 3).unwrap();
        assert_eq!(last3.len(), 3);
        assert_eq!(last3[0].id, 8);
        assert_eq!(last3[1].id, 9);
        assert_eq!(last3[2].id, 10);
    }
}
