use std::path::{Path, PathBuf};

/// The name of the BotBus directory.
pub const BOTBUS_DIR: &str = ".botbus";

/// Find the BotBus project root by walking up from the given path.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        let botbus_dir = current.join(BOTBUS_DIR);
        if botbus_dir.is_dir() {
            return Some(current);
        }

        if !current.pop() {
            return None;
        }
    }
}

/// Get the .botbus directory path for a project.
pub fn botbus_dir(project_root: &Path) -> PathBuf {
    project_root.join(BOTBUS_DIR)
}

/// Get the channels directory path.
pub fn channels_dir(project_root: &Path) -> PathBuf {
    botbus_dir(project_root).join("channels")
}

/// Get the path to a specific channel file.
pub fn channel_path(project_root: &Path, channel: &str) -> PathBuf {
    channels_dir(project_root).join(format!("{}.jsonl", channel))
}

/// Get the agents.jsonl path.
pub fn agents_path(project_root: &Path) -> PathBuf {
    botbus_dir(project_root).join("agents.jsonl")
}

/// Get the claims.jsonl path.
pub fn claims_path(project_root: &Path) -> PathBuf {
    botbus_dir(project_root).join("claims.jsonl")
}

/// Get the state.json path.
pub fn state_path(project_root: &Path) -> PathBuf {
    botbus_dir(project_root).join("state.json")
}

/// Get the index.sqlite path.
pub fn index_path(project_root: &Path) -> PathBuf {
    botbus_dir(project_root).join("index.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_project_root() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        // No .botbus yet
        assert!(find_project_root(root).is_none());

        // Create .botbus
        std::fs::create_dir(root.join(BOTBUS_DIR)).unwrap();
        assert_eq!(find_project_root(root), Some(root.to_path_buf()));

        // Should find from subdirectory
        let subdir = root.join("src").join("deep");
        std::fs::create_dir_all(&subdir).unwrap();
        assert_eq!(find_project_root(&subdir), Some(root.to_path_buf()));
    }

    #[test]
    fn test_path_helpers() {
        let root = Path::new("/project");

        assert_eq!(botbus_dir(root), PathBuf::from("/project/.botbus"));
        assert_eq!(
            channels_dir(root),
            PathBuf::from("/project/.botbus/channels")
        );
        assert_eq!(
            channel_path(root, "general"),
            PathBuf::from("/project/.botbus/channels/general.jsonl")
        );
        assert_eq!(
            agents_path(root),
            PathBuf::from("/project/.botbus/agents.jsonl")
        );
        assert_eq!(
            claims_path(root),
            PathBuf::from("/project/.botbus/claims.jsonl")
        );
        assert_eq!(
            state_path(root),
            PathBuf::from("/project/.botbus/state.json")
        );
        assert_eq!(
            index_path(root),
            PathBuf::from("/project/.botbus/index.sqlite")
        );
    }
}
