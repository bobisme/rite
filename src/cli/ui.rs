use anyhow::Result;

/// Launch the terminal UI.
pub fn run(channel: Option<String>) -> Result<()> {
    crate::tui::run(channel)
}
