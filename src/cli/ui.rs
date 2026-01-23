use anyhow::Result;
use std::path::Path;

/// Launch the terminal UI.
pub fn run(channel: Option<String>, project_root: &Path) -> Result<()> {
    crate::tui::run(project_root, channel)
}
