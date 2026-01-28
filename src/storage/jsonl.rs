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
    if !path.exists() {
        return Ok((Vec::new(), 0));
    }

    let mut file =
        File::open(path).with_context(|| format!("Failed to open file: {}", path.display()))?;

    file.lock_shared()
        .with_context(|| format!("Failed to acquire shared lock on: {}", path.display()))?;

    // Seek to offset
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("Failed to seek in: {}", path.display()))?;

    let reader = BufReader::new(&file);
    let mut records = Vec::new();

    for line_result in reader.lines() {
        let line =
            line_result.with_context(|| format!("Failed to read from: {}", path.display()))?;

        if line.trim().is_empty() {
            continue;
        }

        let record: T = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse in {}: {}", path.display(), line))?;

        records.push(record);
    }

    // Get the new offset
    let mut file = File::open(path)?;
    let new_offset = file.seek(SeekFrom::End(0))?;

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
        .filter_map(|l| l.ok())
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
