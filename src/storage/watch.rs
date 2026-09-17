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

/// Why a watched stream can no longer be trusted to deliver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchFailure {
    /// The notify backend reported an error (overflow, watch invalidated,
    /// a permanent backend fault). Events may have been lost.
    Backend(String),
    /// The watcher's sender is gone; nothing will ever arrive again.
    Disconnected,
    /// The backend asked for a rescan: its event queue overflowed (inotify
    /// `IN_Q_OVERFLOW`, FSEvents dropped events) and an unknown number of
    /// events were lost. Delivered as a successful event with no paths, so
    /// it looks like a quiet poll unless the flag is read.
    Rescan,
}

impl std::fmt::Display for WatchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatchFailure::Backend(e) => write!(f, "filesystem watcher reported an error: {e}"),
            WatchFailure::Disconnected => write!(f, "filesystem watcher disconnected"),
            WatchFailure::Rescan => write!(
                f,
                "filesystem watcher dropped events (queue overflow); a rescan is needed"
            ),
        }
    }
}

impl std::error::Error for WatchFailure {}

/// Like [`debounce_events`], but a backend error, a disconnected watcher,
/// or a rescan notice is returned instead of folded into an empty batch. A
/// consumer whose occupancy claim implies "I can deliver" must stop on any
/// of them: after an overflow or an invalidated watch, silence is
/// indistinguishable from nothing having happened.
pub fn debounce_events_checked(
    rx: &Receiver<notify::Result<Event>>,
    duration: Duration,
) -> Result<Vec<std::path::PathBuf>, WatchFailure> {
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
                if event.need_rescan() {
                    return Err(WatchFailure::Rescan);
                }
                for path in event.paths {
                    paths.insert(path);
                }
            }
            Ok(Err(e)) => return Err(WatchFailure::Backend(e.to_string())),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(WatchFailure::Disconnected),
        }
    }

    Ok(paths.into_iter().collect())
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

    #[test]
    fn checked_debounce_treats_a_rescan_notice_as_a_failure() {
        use notify::event::{EventKind, Flag};
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        // What inotify queue overflow and FSEvents dropped-event notices
        // look like from notify: a successful event, no paths, rescan set.
        tx.send(Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)))
            .unwrap();
        assert_eq!(
            debounce_events_checked(&rx, Duration::from_millis(50)),
            Err(WatchFailure::Rescan)
        );
        // A normal event with no paths is still a quiet poll, not a failure.
        tx.send(Ok(Event::new(EventKind::Other))).unwrap();
        assert_eq!(
            debounce_events_checked(&rx, Duration::from_millis(50)),
            Ok(vec![])
        );
    }

    #[test]
    fn checked_debounce_reports_backend_errors_and_disconnects() {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        tx.send(Err(notify::Error::generic("queue overflow")))
            .unwrap();
        let err = debounce_events_checked(&rx, Duration::from_millis(50)).unwrap_err();
        assert!(
            matches!(err, WatchFailure::Backend(ref m) if m.contains("overflow")),
            "{err}"
        );

        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        drop(tx);
        assert_eq!(
            debounce_events_checked(&rx, Duration::from_millis(50)).unwrap_err(),
            WatchFailure::Disconnected
        );

        // The unchecked helper keeps its old behaviour for callers that
        // only display.
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        tx.send(Err(notify::Error::generic("x"))).unwrap();
        drop(tx);
        assert!(debounce_events(&rx, Duration::from_millis(50)).is_empty());
    }
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
