use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::Path;

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
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open file: {}", path.display()))?;

    // Acquire exclusive lock for atomic read-check-write
    file.lock_exclusive()
        .with_context(|| format!("Failed to acquire lock on: {}", path.display()))?;

    // Read existing records while holding the lock
    let reader = BufReader::new(&file);
    let mut records: Vec<T> = Vec::new();

    for line_result in reader.lines() {
        let line =
            line_result.with_context(|| format!("Failed to read from: {}", path.display()))?;

        if line.trim().is_empty() {
            continue;
        }

        let rec: T = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse in {}: {}", path.display(), line))?;

        records.push(rec);
    }

    // Check if we should append
    if !predicate(&records) {
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
pub fn read_records<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    // Use shared lock for reading
    file.lock_shared()
        .with_context(|| format!("Failed to acquire shared lock on: {}", path.display()))?;

    let reader = BufReader::new(&file);
    let mut records = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result.with_context(|| {
            format!(
                "Failed to read line {} from: {}",
                line_num + 1,
                path.display()
            )
        })?;

        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        let record: T = serde_json::from_str(&line).with_context(|| {
            format!(
                "Failed to parse line {} in {}: {}",
                line_num + 1,
                path.display(),
                line
            )
        })?;

        records.push(record);
    }

    Ok(records)
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
    if !path.exists() {
        return Ok((Vec::new(), 0));
    }

    if limit == Some(0) {
        return Ok((Vec::new(), offset));
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
    let mut new_offset = offset;

    loop {
        let mut line = String::new();
        let bytes_read = reader
            .read_line(&mut line)
            .with_context(|| format!("Failed to read from: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }

        new_offset = reader.stream_position()?;

        if line.trim().is_empty() {
            continue;
        }

        let record: T = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse in {}: {}", path.display(), line))?;

        records.push(record);

        if limit.is_some_and(|limit| records.len() >= limit) {
            break;
        }
    }

    if limit.is_none() {
        // Get the new offset while still holding the shared lock. Reopening
        // after reading would leave a race where a concurrent append could be
        // included in the returned offset without its records being returned.
        new_offset = reader.seek(SeekFrom::End(0))?;
    }

    Ok((records, new_offset))
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
