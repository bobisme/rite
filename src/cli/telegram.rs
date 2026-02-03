use anyhow::{Context, Result};

use crate::core::project::{ensure_cache_dir, telegram_config_path};
use crate::telegram::config::TelegramConfigStore;
use crate::telegram::service;

pub fn run() -> Result<()> {
    ensure_cache_dir()?;
    let store = TelegramConfigStore::new(telegram_config_path());
    let config = store.load()?;

    // Create a tokio runtime just for the telegram service
    let rt = tokio::runtime::Runtime::new().with_context(|| "Failed to create tokio runtime")?;

    rt.block_on(service::run(config, store))
}
