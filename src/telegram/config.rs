use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn default_agent_name() -> String {
    "telegram".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub owner_user_id: i64,
    pub chat_id: i64,
    #[serde(default = "default_agent_name")]
    pub agent_name: String,
    #[serde(default)]
    pub channel_topics: HashMap<String, i64>,
    #[serde(default)]
    pub muted_topics: HashSet<i64>,
    #[serde(default)]
    pub topic_titles: HashMap<i64, String>,
    #[serde(default)]
    pub last_update_id: Option<i64>,
}

impl TelegramConfig {
    pub fn validate(&self) -> Result<()> {
        if self.bot_token.trim().is_empty() {
            bail!("Telegram config missing bot_token");
        }

        if self.owner_user_id == 0 {
            bail!("Telegram config missing owner_user_id");
        }

        if self.chat_id == 0 {
            bail!("Telegram config missing chat_id");
        }

        if self.agent_name.trim().is_empty() {
            bail!("Telegram config missing agent_name");
        }

        Ok(())
    }
}

#[derive(Clone)]
pub struct TelegramConfigStore {
    path: PathBuf,
}

impl TelegramConfigStore {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<TelegramConfig> {
        if !self.path.exists() {
            bail!(
                "Telegram config not found: {}\n\nCreate a JSON file with bot_token, owner_user_id, and chat_id.",
                self.path.display()
            );
        }

        let file = File::open(&self.path)
            .with_context(|| format!("Failed to open telegram config: {}", self.path.display()))?;

        file.lock_shared()
            .with_context(|| "Failed to acquire shared lock on telegram config")?;

        let mut contents = String::new();
        let mut reader = std::io::BufReader::new(&file);
        reader
            .read_to_string(&mut contents)
            .with_context(|| "Failed to read telegram config")?;

        if contents.trim().is_empty() {
            bail!("Telegram config is empty: {}", self.path.display());
        }

        let config: TelegramConfig = serde_json::from_str(&contents)
            .with_context(|| "Failed to parse telegram config JSON")?;

        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, config: &TelegramConfig) -> Result<()> {
        config.validate()?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;

            #[cfg(unix)]
            {
                let permissions = std::fs::Permissions::from_mode(0o700);
                std::fs::set_permissions(parent, permissions).with_context(|| {
                    format!("Failed to set permissions on: {}", parent.display())
                })?;
            }
        }

        // Create file with restrictive permissions atomically (no TOCTOU race)
        #[cfg(unix)]
        let file = {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600) // Set permissions atomically at creation
                .open(&self.path)
                .with_context(|| {
                    format!(
                        "Failed to open telegram config for writing: {}",
                        self.path.display()
                    )
                })?
        };

        #[cfg(not(unix))]
        let file = {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)
                .with_context(|| {
                    format!(
                        "Failed to open telegram config for writing: {}",
                        self.path.display()
                    )
                })?
        };

        file.lock_exclusive()
            .with_context(|| "Failed to acquire exclusive lock on telegram config")?;

        let json = serde_json::to_string_pretty(config)
            .with_context(|| "Failed to serialize telegram config")?;

        let mut writer = std::io::BufWriter::new(&file);
        writer
            .write_all(json.as_bytes())
            .with_context(|| "Failed to write telegram config")?;
        writer.flush()?;
        file.sync_all()?;

        Ok(())
    }
}
