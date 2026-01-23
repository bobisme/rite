use anyhow::Result;
use std::path::Path;

pub struct SearchOptions {
    pub query: String,
    pub channel: Option<String>,
    pub count: usize,
    pub from: Option<String>,
}

pub fn run(_options: SearchOptions, _project_root: &Path) -> Result<()> {
    todo!("Implement search command")
}
