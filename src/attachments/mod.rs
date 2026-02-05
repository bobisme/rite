//! Unified content-addressed attachment cache.
//!
//! All attachment sources (Telegram downloads, CLI `--attach`, agent APIs)
//! store files in a single flat directory keyed by SHA256 hash, providing
//! automatic deduplication and simple lookups.

pub mod cache;
pub mod metadata;

pub use cache::{AttachmentCache, AttachmentSource, CleanupStats, StoredAttachment};
pub use metadata::AttachmentMetadata;

use crate::core::project::data_dir;
use std::path::PathBuf;

/// Get the default attachments cache directory.
pub fn attachments_dir() -> PathBuf {
    data_dir().join("attachments")
}
