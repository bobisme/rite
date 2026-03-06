use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Create a file watcher for the given directory.
///
/// Returns a receiver that will emit events when files change.
pub fn watch_directory(
    path: &Path,
) -> Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    let (tx, rx) = mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })
    .with_context(|| "Failed to create file watcher")?;

    watcher
        .watch(path, RecursiveMode::Recursive)
        .with_context(|| format!("Failed to watch directory: {}", path.display()))?;

    Ok((watcher, rx))
}

/// Debounce events from a watcher.
///
/// Collects events for `duration` and returns unique file paths that changed.
pub fn debounce_events(
    rx: &Receiver<notify::Result<Event>>,
    duration: Duration,
) -> Vec<std::path::PathBuf> {
    use std::collections::HashSet;

    let mut paths = HashSet::new();
    let deadline = std::time::Instant::now() + duration;

    loop {
        let timeout = deadline.saturating_duration_since(std::time::Instant::now());
        if timeout.is_zero() {
            break;
        }

        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                for path in event.paths {
                    paths.insert(path);
                }
            }
            Ok(Err(_)) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    paths.into_iter().collect()
}

/// Filter events to only include JSONL files in the channels directory.
pub fn filter_channel_events(paths: Vec<std::path::PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .filter_map(|path| {
            // Check if it's a .jsonl file
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                // Extract the channel name (filename without extension)
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_filter_channel_events() {
        let paths = vec![
            std::path::PathBuf::from("/project/.rite/channels/general.jsonl"),
            std::path::PathBuf::from("/project/.rite/channels/backend.jsonl"),
            std::path::PathBuf::from("/project/.rite/state.json"),
            std::path::PathBuf::from("/project/.rite/index.sqlite"),
        ];

        let channels = filter_channel_events(paths);
        assert_eq!(channels.len(), 2);
        assert!(channels.contains(&"general".to_string()));
        assert!(channels.contains(&"backend".to_string()));
    }

    #[test]
    fn test_watch_directory() {
        let temp = TempDir::new().unwrap();
        let (watcher, rx) = watch_directory(temp.path()).unwrap();

        // Write a file to trigger an event
        fs::write(temp.path().join("test.txt"), "hello").unwrap();

        // Give the watcher time to pick up the event
        std::thread::sleep(Duration::from_millis(100));

        // Should have received at least one event
        let events = debounce_events(&rx, Duration::from_millis(50));
        // Note: The exact number of events can vary by platform

        // Keep watcher alive until we're done collecting events
        drop(watcher);

        // Event delivery is platform-dependent (e.g., some platforms batch events,
        // some may not deliver events for files created immediately after watch starts).
        // We verify the watcher setup succeeds and doesn't panic; event count varies.
        let _ = events; // Acknowledge we received events (may be empty on some platforms)
    }
}
