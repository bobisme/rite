//! Sidecar metadata for cached attachments.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Metadata stored alongside each cached attachment as `{hash}.{ext}.meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    /// Original filename as provided by the source
    pub original_filename: String,
    /// Detected MIME type (via magic numbers, not extension)
    pub mime_type: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// SHA256 hash of the file content
    pub sha256: String,
    /// When the file was stored in cache
    pub stored_at: DateTime<Utc>,
    /// Who stored this (agent name or "telegram-daemon")
    pub stored_by: String,
    /// Source of the attachment: "telegram", "cli", "agent"
    pub source: String,
    /// Telegram file_id (only if from Telegram)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_file_id: Option<String>,
    /// Telegram file_unique_id (only if from Telegram)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telegram_file_unique_id: Option<String>,
    /// Source message ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<String>,
    /// Source channel
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_channel: Option<String>,
}
