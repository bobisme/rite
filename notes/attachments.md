# Rite ↔ Telegram Attachments Bridge Design

## Overview

This document outlines the design for bridging file attachments between Rite messages and Telegram. Currently, Rite supports three attachment types (File, Inline, Url) but the Telegram integration only displays attachment names as text. This design proposes a two-way attachment bridge that:

- Sends Rite attachments to Telegram users (with smart MIME type detection)
- Captures Telegram attachments (photos, documents, audio, video) and bridges them back to Rite
- Uses unified content-addressed storage for all attachments (Telegram, CLI `rite send --attach`, agent APIs)
- Handles storage, size limits, and format restrictions appropriately

**Key Design Decisions**:
- **Content-addressed storage**: Files stored by SHA256 hash (automatic deduplication)
- **Sidecar metadata**: Each file has a `.meta.json` for Telegram file_id, source info, etc.
- **Daemon-managed**: Telegram daemon handles all downloads (not individual agents)
- **MIME detection**: Use `infer` crate for magic-number detection (not extension-based)
- **No core Message changes**: Keep using `File { path }` attachment type, no Telegram-specific variants

## Current State

### Rite Attachment System

The `Message` struct in `/src/core/message.rs` includes:
- `attachments: Vec<Attachment>` - array of file attachments
- `Attachment` struct with name and content type

Attachment types:
1. **File**: Reference to file path (relative to project root)
   - Use case: Code snippets, configs, logs
   - Example: `{ type: "file", path: "src/config.rs" }`

2. **Inline**: Text content with optional language hint
   - Use case: Code snippets, formatted text
   - Example: `{ type: "inline", content: "fn main() {}", language: "rust" }`

3. **Url**: URL reference
   - Use case: Links, documentation, external resources
   - Example: `{ type: "url", url: "https://example.com/docs" }`

### Current Telegram Integration

The telegram service (`/src/telegram/service.rs`):
- **Bus → Telegram**: Formats messages with attachment names only (line 449-451)
- **Telegram → Bus**: Ignores attachments; accepts only text messages (line 155-157)
- Size limit: 4000 characters per Telegram message (TELEGRAM_MAX_CHARS)
- Uses Telegram forum topics for channel mapping

## Telegram Bot API Capabilities

### Supported Media Types

Telegram Bot API supports these media types in messages:

| Type | Method | Max Size | Format |
|------|--------|----------|--------|
| **Photo** | `sendPhoto` | 10 MB | JPG, PNG, WebP, GIF |
| **Document** | `sendDocument` | 50 MB | Any file type |
| **Audio** | `sendAudio` | 50 MB | MP3, WAV, FLAC |
| **Video** | `sendVideo` | 50 MB | MP4, WebM, MKV |
| **Voice** | `sendVoice` | 20 MB | OGG/OPUS (3.5 MB), AMR |
| **Video Note** | `sendVideoNote` | 50 MB | MP4, WebM (≤1 min) |
| **Animation** | `sendAnimation` | 50 MB | GIF, H.264/VP9 MP4 |

### Upload Methods

1. **File ID**: Reference previously uploaded files (instant, no re-upload)
2. **File Path**: Upload from server filesystem
3. **Multipart Form**: Upload file data directly from request body
4. **URL**: Telegram downloads and caches the file

### Receiving Media

When Telegram users send media:
- API returns `File` object with `file_id`, `file_unique_id`, and optional `file_path`
- Use `getFile` method to retrieve `file_path` (temporary 1-hour URL)
- Download via `https://api.telegram.org/file/bot<TOKEN>/<file_path>`

## Bus → Telegram Flow

### Architecture Decision: Hybrid Approach

**Principle**: Different attachment types use different strategies.

### Implementation Details

#### 1. **Inline Attachments** (Text with language hints)

**Approach**: Send as formatted Telegram messages

```
Format in Telegram:
┌─ From: alice
├─ Code snippet (language: rust)
└─ [source code formatted in code block]
```

**Implementation**:
- Detect language hint, use Telegram `<code>` or `<pre>` HTML tags
- If text > 4000 chars, send as document (raw text file)
- **Risk**: Inline content with newlines may exceed Telegram limit

**Fallback**:
- For large inline content (>4000 chars), save as temporary `.txt` file and send as document
- Auto-clean temp files after 1 hour

#### 2. **File Attachments** (Local filesystem references)

**Approach**: Read file and upload, or send as document

```
Flow:
Rite file:src/config.rs
  ↓
Check file size & type
  ↓
If binary/non-text → Send as Document
If text + small → Format in message
If text + large → Send as Document
```

**Implementation**:
- Check if file path is relative (resolve to workspace root) or absolute
- For text files: include in message body with syntax highlighting if small (<1000 lines)
- For binary/large: Send as `sendDocument` for source code, or media type (photo, video, etc.)
- **Size check**: If file > 50 MB, error gracefully with message

**File Type Detection** (using `infer` crate):
```rust
// Read file bytes and detect MIME type via magic numbers
let bytes = fs::read(&path)?;
let mime_type = infer::get(&bytes)
    .map(|t| t.mime_type())
    .unwrap_or("application/octet-stream");

// Route based on detected MIME type
match mime_type {
    "image/jpeg" | "image/png" | "image/webp" | "image/gif" => {
        telegram_client.send_photo(chat_id, thread_id, path, caption).await?;
    }
    "video/mp4" | "video/webm" | "video/x-matroska" => {
        telegram_client.send_video(chat_id, thread_id, path, caption).await?;
    }
    "audio/mpeg" | "audio/wav" | "audio/flac" => {
        telegram_client.send_audio(chat_id, thread_id, path, caption).await?;
    }
    _ => {
        // Everything else (including text/code) as document
        telegram_client.send_document(chat_id, thread_id, path, caption).await?;
    }
}
```

**Benefits**:
- Detects real file type (not fooled by wrong extensions like `.jpg.exe`)
- Handles files without extensions
- Validates against malicious files (e.g., executable disguised as image)

**Risk**: File may not exist or be readable → send error message to Telegram

#### 3. **URL Attachments**

**Approach**: Include as clickable links in Telegram message

```
Format in Telegram:
From: bob
Check the docs: [docs](https://example.com/docs)
```

**Implementation**:
- Embed URL in message body using Markdown `[text](url)` format
- Telegram client will render as clickable link
- No file transfer needed

### Attachment Metadata Preservation

**Challenge**: Telegram doesn't preserve "attachment name" concept

**Solution**: Use message formatting to encode metadata

```json
{
  "type": "telegram_attachment",
  "name": "config.rs",
  "source_type": "file",
  "telegram_file_id": "AgAD..."
}
```

When publishing to Telegram, include in caption:
```
Attachment: config.rs (type: file)
```

## Telegram → Bus Flow

### Message Handling

Current behavior (line 155-157 in service.rs): Ignore all non-text messages.

**New behavior**: Telegram daemon extracts media, downloads to cache, creates Rite attachments

### Daemon Download Implementation

```rust
// In src/telegram/service.rs

async fn handle_telegram_media(
    &self,
    file_id: &str,
    file_unique_id: &str,
    original_filename: Option<&str>,
    message_id: &str,
    channel: &str,
) -> Result<Attachment> {
    // 1. Download file from Telegram
    let bytes = self.client.download_file(file_id).await?;

    // 2. Compute hash
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = format!("{:x}", hasher.finalize());

    // 3. Detect real MIME type (don't trust Telegram)
    let mime_type = infer::get(&bytes)
        .map(|t| t.mime_type())
        .unwrap_or("application/octet-stream");

    // 4. Determine extension
    let ext = mime2ext::mime_to_ext(mime_type).unwrap_or("bin");
    let final_path = self.cache_dir.join(format!("{}.{}", hash, ext));

    // 5. Write file atomically (if not exists)
    if !final_path.exists() {
        let tmp_path = self.cache_dir.join(format!("{}.tmp", hash));
        fs::write(&tmp_path, bytes)?;
        fs::rename(&tmp_path, &final_path)?;

        // 6. Write metadata
        let meta = AttachmentMetadata {
            original_filename: original_filename.unwrap_or("file").to_string(),
            mime_type: mime_type.to_string(),
            size_bytes: bytes.len() as u64,
            sha256: hash.clone(),
            downloaded_at: Utc::now(),
            downloaded_by: "telegram-daemon".to_string(),
            source: "telegram".to_string(),
            telegram_file_id: Some(file_id.to_string()),
            telegram_file_unique_id: Some(file_unique_id.to_string()),
            source_message_id: Some(message_id.to_string()),
            source_channel: Some(channel.to_string()),
            source_project: Some(self.project.clone()),
        };

        let meta_path = final_path.with_extension(format!("{}.meta.json", ext));
        let meta_tmp = self.cache_dir.join(format!("{}.meta.tmp", hash));
        fs::write(&meta_tmp, serde_json::to_string_pretty(&meta)?)?;
        fs::rename(&meta_tmp, &meta_path)?;
    }

    // 7. Return attachment reference
    Ok(Attachment {
        name: original_filename.unwrap_or("file").to_string(),
        content: AttachmentContent::File {
            path: final_path.to_string_lossy().to_string(),
        },
    })
}
```

**Error Handling**:
- If download fails, retry once with exponential backoff
- If still fails, publish message without attachment + log error
- If disk full during write, cleanup cache and retry
- If hash collision detected (astronomically rare), append counter to filename

### Supported Telegram Media → Rite Attachment Mapping

| Telegram Type | Rite Type | Strategy |
|---------------|-------------|----------|
| **Photo** | File | Download and store with unique name |
| **Document** | File | Download and store with original filename |
| **Audio** | File | Download and store with `.mp3` or original ext |
| **Video** | File | Download and store with `.mp4` or original ext |
| **Voice Note** | File | Download and store as `.ogg` |
| **Animation** (GIF) | File | Download and store as `.mp4` or `.gif` |
| **Text** | Inline | Already handled; create inline attachment if large |

### Implementation Flow

**Key principle**: Telegram daemon handles all downloads synchronously before publishing message to Rite.

```
Telegram Update received
  ↓
Extract media (photo, document, video, etc.)
  ↓
If media present:
   Telegram daemon downloads file immediately
   ↓
   Compute SHA256 hash
   ↓
   Detect MIME type (infer crate)
   ↓
   Store as {hash}.{ext} in cache (if not exists)
   ↓
   Write {hash}.{ext}.meta.json with Telegram file_id
   ↓
   Create Attachment::File with cache path
  ↓
Create Rite message with attachments
  ↓
Publish to Rite channels
```

**Why daemon downloads**:
- Telegram file download URLs expire in 1 hour
- Agents may not process messages immediately
- Centralized download = single point of failure handling, rate limiting, retry logic
- Deduplication happens automatically (same hash = already cached)

### Storage Strategy

**Challenge**: Where to store downloaded Telegram files? How to handle `rite send --attach`?

**Design Decision**: Unified content-addressed attachment cache

**Location**: `~/.local/share/rite/attachments/`

**Directory Structure** (flat, hash-based):
```
~/.local/share/rite/attachments/
├── a3f8b9c2...d4e5.jpg
├── a3f8b9c2...d4e5.jpg.meta.json
├── f7e6d5c4...a1b2.pdf
└── f7e6d5c4...a1b2.pdf.meta.json
```

**File Naming Convention**:
- Files: `{sha256-hash}.{extension}`
- Metadata: `{sha256-hash}.{extension}.meta.json`
- Extension determined by MIME type detection (via `infer` crate)

**Benefits**:
- **Automatic deduplication**: Same file uploaded twice = same hash, stored once
- **Simple lookups**: Hash → file is O(1) filesystem read
- **No collisions**: SHA256 ensures uniqueness (collision astronomically rare)
- **Source-agnostic**: Works for Telegram downloads, `rite send --attach`, agent APIs
- **Easy cleanup**: Age-based on `mtime`, no complex queries needed
- **Debuggable**: Just `cat {hash}.meta.json` to inspect

**Metadata Schema** (`.meta.json`):
```json
{
  "original_filename": "screenshot.jpg",
  "mime_type": "image/jpeg",
  "size_bytes": 245680,
  "sha256": "a3f8b9c2...d4e5",
  "downloaded_at": "2026-02-05T14:23:45Z",
  "downloaded_by": "telegram-daemon",
  "source": "telegram",
  "telegram_file_id": "AgAD...",           // Only if from Telegram
  "telegram_file_unique_id": "AQADy...",  // Only if from Telegram
  "source_message_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  "source_channel": "general",
  "source_project": "rite"
}
```

**Lifecycle**:
1. **Download/Copy**: Telegram daemon or `rite send --attach` writes file to cache
2. **Hash**: Compute SHA256 of content
3. **Detect MIME**: Use `infer` crate to detect real file type (not extension)
4. **Store**: Write `{hash}.{ext}` and `{hash}.{ext}.meta.json` atomically
5. **Reference**: Store path in Rite `Attachment::File { path: "~/.local/share/rite/attachments/{hash}.{ext}" }`
6. **Cleanup**: Age-based removal (files older than 7 days, configurable)

**Size Management**:
- Max cache size: 500 MB (configurable)
- When exceeded: Delete oldest files by `mtime` until under threshold
- Per-attachment limit: 50 MB (Telegram max)

**Atomic Writes**:
```rust
// Write to .tmp file, rename atomically
let tmp_path = cache_dir.join(format!("{}.tmp", hash));
fs::write(&tmp_path, bytes)?;
fs::rename(&tmp_path, &final_path)?;

// Write metadata similarly
let meta_tmp = cache_dir.join(format!("{}.{}.meta.json.tmp", hash, ext));
fs::write(&meta_tmp, serde_json::to_string_pretty(&meta)?)?;
fs::rename(&meta_tmp, &meta_path)?;
```

## Unified Attachment Storage

All attachment sources (Telegram, `rite send --attach`, agent APIs) use the same storage mechanism:

### `rite send --attach` Integration

When users run `rite send --attach <file>`, Rite should:

1. **Read file** from provided path
2. **Compute hash** (SHA256)
3. **Detect MIME type** (using `infer` crate)
4. **Copy to cache** as `{hash}.{ext}` if not already present
5. **Write metadata** as `{hash}.{ext}.meta.json`
6. **Create message** with `Attachment::File { path: "~/.local/share/rite/attachments/{hash}.{ext}" }`

**Example**:
```bash
rite send general --attach ./screenshot.png "Here's the bug"
```

**Internals**:
```rust
// 1. Read file
let bytes = fs::read("./screenshot.png")?;

// 2. Hash
let hash = sha256(&bytes);

// 3. Detect MIME and extension
let mime_type = infer::get(&bytes)
    .map(|t| t.mime_type())
    .unwrap_or("application/octet-stream");
let ext = mime2ext::mime_to_ext(mime_type).unwrap_or("bin");

// 4. Store if not exists
let cache_path = cache_dir.join(format!("{}.{}", hash, ext));
if !cache_path.exists() {
    fs::write(&cache_path, bytes)?;

    // 5. Write metadata
    let meta = AttachmentMetadata {
        original_filename: "screenshot.png".to_string(),
        mime_type: mime_type.to_string(),
        size_bytes: bytes.len() as u64,
        sha256: hash.clone(),
        downloaded_at: Utc::now(),
        downloaded_by: env::var("RITE_AGENT").unwrap_or_else(|_| "cli".to_string()),
        source: "cli".to_string(),
        telegram_file_id: None,
        telegram_file_unique_id: None,
        source_message_id: None,
        source_channel: Some("general".to_string()),
        source_project: None,
    };
    fs::write(cache_path.with_extension(format!("{}.meta.json", ext)),
              serde_json::to_string_pretty(&meta)?)?;
}

// 6. Create attachment reference
let attachment = Attachment {
    name: "screenshot.png".to_string(),
    content: AttachmentContent::File {
        path: cache_path.to_string_lossy().to_string(),
    },
};
```

### Path Representation

**Storage**: Always absolute paths in cache directory
```
/home/user/.local/share/rite/attachments/a3f8b9c2...d4e5.jpg
```

**In Messages**: Store absolute path in `Attachment::File { path }`
```rust
AttachmentContent::File {
    path: "/home/user/.local/share/rite/attachments/a3f8b9c2...d4e5.jpg"
}
```

**Display**: Show original filename from metadata when rendering
```
📎 screenshot.jpg (245 KB)
```

## Size Limits and Quotas

### Per-Attachment Limits

| Limit | Value | Reasoning |
|-------|-------|-----------|
| **Telegram upload max** | 50 MB | Telegram Bot API hard limit |
| **Rite message display** | 4000 chars | Telegram message limit |
| **Inline content max** | 10 KB | Rite storage efficiency |
| **File attachment max** | 50 MB | Same as Telegram |
| **Voice/Audio max** | 20 MB | Telegram voice limit |

### Message-Level Limits

- **Max attachments per message**: 10
- **Max total attachment size per message**: 100 MB
- **Max total attachment size in memory**: 512 MB (prevent OOM)

### Cache Limits

- **Cache directory max size**: 500 MB (configurable)
- **Age-based cleanup**: Files older than 7 days (configurable)
- **Cache flush**: Automatic when size > threshold

### Cache Cleanup Implementation

```rust
// In src/attachments/cache.rs

pub fn cleanup_cache(cache_dir: &Path, max_age_days: u64, max_size_mb: u64) -> Result<()> {
    let cutoff = SystemTime::now() - Duration::from_secs(max_age_days * 24 * 60 * 60);
    let max_bytes = max_size_mb * 1024 * 1024;

    // 1. Collect all files with their metadata
    let mut entries = Vec::new();
    for entry in fs::read_dir(cache_dir)? {
        let entry = entry?;
        let path = entry.path();

        // Skip metadata files and temp files
        if path.extension().and_then(|s| s.to_str()) == Some("json")
            || path.file_name().and_then(|s| s.to_str()).unwrap_or("").contains(".tmp") {
            continue;
        }

        let metadata = entry.metadata()?;
        entries.push((path, metadata));
    }

    // 2. Age-based cleanup (delete files older than cutoff)
    for (path, metadata) in &entries {
        if metadata.modified()? < cutoff {
            fs::remove_file(path)?;
            // Also remove metadata file
            let meta_path = path.with_extension(
                format!("{}.meta.json", path.extension().and_then(|s| s.to_str()).unwrap_or(""))
            );
            let _ = fs::remove_file(meta_path); // Ignore if doesn't exist
        }
    }

    // 3. Size-based cleanup (if still over limit, delete oldest first)
    let total_size: u64 = entries.iter()
        .filter(|(path, _)| path.exists())
        .map(|(_, metadata)| metadata.len())
        .sum();

    if total_size > max_bytes {
        // Sort by modification time (oldest first)
        entries.sort_by_key(|(_, metadata)| metadata.modified().ok());

        let mut freed = 0u64;
        for (path, metadata) in entries {
            if total_size - freed <= max_bytes {
                break;
            }

            let size = metadata.len();
            fs::remove_file(&path)?;
            let meta_path = path.with_extension(
                format!("{}.meta.json", path.extension().and_then(|s| s.to_str()).unwrap_or(""))
            );
            let _ = fs::remove_file(meta_path);

            freed += size;
        }
    }

    Ok(())
}
```

**Cleanup triggers**:
- On daemon startup (clean stale files)
- Periodically (every hour) in background thread
- Before writing new file (if cache near limit)

**Orphan cleanup**:
```rust
// Remove .meta.json files without corresponding data files
for entry in glob(cache_dir.join("*.meta.json"))? {
    let data_file = entry.with_extension("");
    if !data_file.exists() {
        fs::remove_file(entry)?;
    }
}
```

### Validation Rules

When receiving from Telegram:
```rust
if attachment.size_bytes > 50 * 1024 * 1024 {
    // Send error message
    return Err("Attachment too large (max 50 MB)".into());
}

if message.attachments.len() > 10 {
    return Err("Too many attachments (max 10)".into());
}

if total_size > 100 * 1024 * 1024 {
    return Err("Total attachment size exceeds 100 MB".into());
}
```

## API Design

### Module Structure

```
src/
├── attachments/
│   ├── mod.rs          # Public API
│   ├── cache.rs        # AttachmentCache implementation
│   └── metadata.rs     # AttachmentMetadata struct
├── telegram/
│   ├── client.rs       # TelegramClient with download/upload
│   └── service.rs      # Daemon handles media in updates
└── cli/
    └── send.rs         # `rite send --attach` implementation
```

### `src/attachments/mod.rs`

```rust
pub struct AttachmentCache {
    cache_dir: PathBuf,
    max_size_mb: u64,
    max_age_days: u64,
}

impl AttachmentCache {
    pub fn new(cache_dir: PathBuf) -> Result<Self>;

    /// Store bytes in cache, return path and metadata
    pub async fn store(
        &self,
        bytes: Vec<u8>,
        original_filename: &str,
        source: AttachmentSource,
    ) -> Result<StoredAttachment>;

    /// Get file path by hash
    pub fn get(&self, hash: &str) -> Option<PathBuf>;

    /// Read metadata for a cached file
    pub fn read_metadata(&self, hash: &str) -> Result<AttachmentMetadata>;

    /// Cleanup old/excess files
    pub fn cleanup(&self) -> Result<CleanupStats>;
}

pub struct StoredAttachment {
    pub path: PathBuf,
    pub hash: String,
    pub mime_type: String,
    pub size_bytes: u64,
}

pub enum AttachmentSource {
    Telegram {
        file_id: String,
        file_unique_id: String,
        message_id: String,
        channel: String,
        project: String,
    },
    Cli {
        agent: String,
        channel: String,
    },
    Agent {
        agent: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub original_filename: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub downloaded_at: DateTime<Utc>,
    pub downloaded_by: String,
    pub source: String,
    pub telegram_file_id: Option<String>,
    pub telegram_file_unique_id: Option<String>,
    pub source_message_id: Option<String>,
    pub source_channel: Option<String>,
    pub source_project: Option<String>,
}
```

### `src/telegram/client.rs`

```rust
impl TelegramClient {
    /// Download file from Telegram by file_id
    pub async fn download_file(&self, file_id: &str) -> Result<Vec<u8>> {
        // 1. Call getFile API to get file_path
        // 2. Download from https://api.telegram.org/file/bot<TOKEN>/<file_path>
        // 3. Return bytes
    }

    /// Send photo to Telegram
    pub async fn send_photo(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        // Multipart upload with file
    }

    /// Send document to Telegram
    pub async fn send_document(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        // Multipart upload with file
    }

    /// Send video to Telegram
    pub async fn send_video(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        // Multipart upload with file
    }

    /// Send audio to Telegram
    pub async fn send_audio(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        path: &Path,
        caption: Option<&str>,
    ) -> Result<()> {
        // Multipart upload with file
    }
}
```

### `src/telegram/service.rs`

```rust
impl TelegramService {
    async fn handle_update(&self, update: Update) -> Result<()> {
        // Extract media from update
        let media = extract_media(&update)?;

        let attachments = if let Some(media) = media {
            // Download and cache via AttachmentCache
            let stored = self.attachment_cache.store(
                bytes,
                media.filename,
                AttachmentSource::Telegram { /* ... */ },
            ).await?;

            vec![Attachment {
                name: media.filename,
                content: AttachmentContent::File {
                    path: stored.path.to_string_lossy().to_string(),
                },
            }]
        } else {
            vec![]
        };

        // Create and publish message
        let message = Message {
            body: update.message.text,
            attachments,
            // ...
        };
        self.publish_message(message).await?;
    }

    async fn publish_message(&self, message: &Message) -> Result<()> {
        // For each attachment, read file and send to Telegram
        for attachment in &message.attachments {
            match &attachment.content {
                AttachmentContent::File { path } => {
                    let bytes = fs::read(path)?;
                    let mime_type = infer::get(&bytes)
                        .map(|t| t.mime_type())
                        .unwrap_or("application/octet-stream");

                    // Route based on MIME type
                    match mime_type {
                        "image/jpeg" | "image/png" => {
                            self.client.send_photo(chat_id, thread_id, Path::new(path), Some(&attachment.name)).await?;
                        }
                        "video/mp4" => {
                            self.client.send_video(chat_id, thread_id, Path::new(path), Some(&attachment.name)).await?;
                        }
                        // ... other types
                        _ => {
                            self.client.send_document(chat_id, thread_id, Path::new(path), Some(&attachment.name)).await?;
                        }
                    }
                }
                AttachmentContent::Inline { content, language } => {
                    // Format as code block or send as document
                }
                AttachmentContent::Url { url } => {
                    // Embed in message body
                }
            }
        }
    }
}
```

### Message Formatting for Attachments

**Bus → Telegram caption format**:

When sending files to Telegram, use the original filename in the caption:

```
alice: Check the deploy logs

📎 deploy.log
```

Telegram displays the filename automatically, but we include it in caption for consistency.

### Core `Message` Struct - No Changes Needed

**Keep it simple**: Use existing `AttachmentContent::File { path }` for all cached attachments.

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentContent {
    File { path: String },              // Already exists
    Inline { content: String, language: Option<String> },  // Already exists
    Url { url: String },                // Already exists
}
```

**No Telegram-specific variants needed** - the `.meta.json` sidecar stores Telegram file_id and source information.

**Example message with Telegram attachment**:
```json
{
  "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  "channel": "general",
  "agent": "telegram-user-alice",
  "body": "Check this screenshot",
  "attachments": [
    {
      "name": "screenshot.jpg",
      "type": "file",
      "path": "/home/user/.local/share/rite/attachments/a3f8b9c2...d4e5.jpg"
    }
  ]
}
```

The attachment metadata lives in `/home/user/.local/share/rite/attachments/a3f8b9c2...d4e5.jpg.meta.json`:
```json
{
  "original_filename": "screenshot.jpg",
  "mime_type": "image/jpeg",
  "source": "telegram",
  "telegram_file_id": "AgAD...",
  "telegram_file_unique_id": "AQADy..."
}
```

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# Existing dependencies...

# For MIME type detection via magic numbers
infer = "0.16"

# For SHA256 hashing
sha2 = "0.10"

# For MIME type to extension mapping
mime2ext = "0.1"

# Already have these:
# serde = { version = "1.0", features = ["derive"] }
# serde_json = "1.0"
# tokio = { version = "1", features = ["fs", "io-util"] }
# chrono = { version = "0.4", features = ["serde"] }
```

## Implementation Phases

### Phase 1: Foundation - Cache Infrastructure

1. Create `src/attachments/mod.rs` module
2. Implement content-addressed storage (hash-based naming)
3. Add `AttachmentMetadata` struct and serde serialization
4. Implement atomic file writes (tmp → rename)
5. Add cache cleanup utilities (age-based, size-based)
6. Write unit tests for storage and cleanup

**Deliverables**:
- `AttachmentCache` struct with `store()`, `get()`, `cleanup()` methods
- Metadata sidecar JSON schema
- Cache cleanup logic
- Tests for hash collisions, orphan cleanup, size limits

### Phase 2: Telegram Downloads (Telegram → Bus)

1. Extend `TelegramClient::download_file()` in `src/telegram/client.rs`
2. Add MIME detection via `infer` crate
3. Update `handle_update()` to extract media from Telegram updates
4. Integrate `AttachmentCache::store()` in telegram daemon
5. Create `Attachment::File` references with cache paths
6. Handle Telegram API errors (rate limits, timeouts, not found)

**Deliverables**:
- Telegram photos, documents, videos downloaded to cache
- Metadata with `telegram_file_id` for reference
- Messages with file attachments published to Rite
- Error handling for download failures

### Phase 3: Telegram Uploads (Bus → Telegram)

1. Add `send_photo()`, `send_video()`, `send_audio()`, `send_document()` to `TelegramClient`
2. Update `publish_message()` in telegram service to send attachments
3. Read `Attachment::File` paths and detect MIME type
4. Route to appropriate Telegram send method based on MIME
5. Handle inline attachments (format as code blocks or send as documents)
6. Handle URL attachments (embed as links)

**Deliverables**:
- Rite file attachments sent to Telegram as appropriate media types
- Inline code formatted with syntax highlighting
- URLs rendered as clickable links
- Captions with original filenames

### Phase 4: CLI Integration (`rite send --attach`)

1. Add `--attach <file>` flag to `rite send` command
2. Implement file reading and cache storage
3. Use same `AttachmentCache` as Telegram daemon
4. Support multiple attachments per message
5. Validate file size limits

**Deliverables**:
- `rite send general --attach ./file.txt "message"` works
- Files copied to cache with proper metadata
- Deduplication works (same file = same hash)
- Error messages for missing files, size limits

### Phase 5: Testing & Polish

1. End-to-end tests (Telegram → Bus → Telegram round-trip)
2. Integration tests for cache cleanup
3. Load testing (concurrent downloads, many files)
4. Edge cases (network failures, corrupted downloads, disk full)
5. Documentation and examples
6. Performance profiling and optimization

**Deliverables**:
- Integration test suite
- Documented error codes and handling
- Performance benchmarks
- User guide with examples

## Open Questions & Future Work

### Questions

1. **Telegram Groups vs Supergroups**: Current design assumes forum-enabled supergroup. What about regular groups?
   - **Answer**: Require forum-enabled supergroup for attachment support; gracefully degrade for regular groups

2. **File ID Scope**: File IDs are bot-specific. What if bot token changes?
   - **Answer**: File IDs in `TelegramMedia` become invalid; rely on cache_path. Consider migration strategy.

3. **Concurrent Downloads**: How many Telegram files can download simultaneously?
   - **Answer**: Default 5 concurrent (configurable); respects Telegram rate limits

4. **Cache Persistence**: Should cache survive bot restart?
   - **Answer**: Yes, cache is durable on disk; cleanup is periodic (7-day TTL)

5. **User Quota**: Should individual users have attachment quotas?
   - **Answer**: Future feature; currently global cache limit only

### Future Enhancements

1. **Thumbnail Support**: Telegram supports thumbnail requests for photos/videos
   - Generate thumbnails for display in TUI or web UI

2. **Virus Scanning**: Scan downloaded files for malware
   - Integrate with ClamAV or similar before caching

3. **Document Preview**: Generate text preview of documents
   - OCR for images, text extraction from PDFs

4. **Smart Compression**: Compress files before sending to Telegram
   - Auto-compress large images to stay under limits

5. **Caption Formatting**: Rich captions with markdown or HTML
   - Display metadata in formatted captions

6. **Archive Mode**: Move old attachments to cold storage
   - After 30 days, move to archive; keep only recent files hot

## Compatibility Notes

### With Current Rite Features

- **Labels**: Attachment type labels compatible with existing filtering
- **DMs**: Attachments work in direct messages (no topic ID)
- **Message Search**: Attachment names indexed with message body
- **Claims**: No changes needed; attachments are immutable once sent

### Backward Compatibility

- **Old attachment types**: Continue to work unchanged
- **Messages without attachments**: No impact
- **Bot tokens**: File IDs invalidated; cache provides fallback
- **JSONL format**: New `TelegramMedia` type serializes cleanly

## Error Handling Strategy

### Telegram API Failures

| Error | Handling |
|-------|----------|
| **Rate limited** | Exponential backoff (1s → 30s max) |
| **File not found** | Retry once; skip on second failure |
| **File too large** | User message with size info and max limit |
| **Network timeout** | Retry with backoff; eventually skip |
| **Invalid file_id** | Fall back to cache if available; error if not |

### Local File Issues

| Error | Handling |
|-------|----------|
| **File not readable** | Send error message to Telegram; skip attachment |
| **Disk full** | Error message; try cleanup cache |
| **Path traversal** | Validate and reject malicious paths |
| **Symlink loops** | Reject symbolic links to prevent DoS |

### Validation

```rust
fn validate_attachment_path(path: &Path) -> Result<()> {
    // Reject absolute paths outside safe directories
    // Reject symlinks
    // Reject paths with .. traversal
    // Reject very long paths (>1000 chars)
}
```

### Edge Cases

**Hash Collision** (astronomically rare with SHA256):
```rust
let cache_path = cache_dir.join(format!("{}.{}", hash, ext));
if cache_path.exists() {
    let existing_bytes = fs::read(&cache_path)?;
    if existing_bytes != new_bytes {
        // True collision! Append counter
        let cache_path = cache_dir.join(format!("{}-1.{}", hash, ext));
    } else {
        // Same file already cached, skip write
    }
}
```

**Concurrent Downloads** (same file from multiple Telegram messages):
- Write to `.tmp` file, rename atomically
- If rename fails, check if final file exists
- If exists with same hash, that's fine (another download finished first)
- Metadata may differ (different source messages), last-write-wins acceptable

**Orphaned Metadata**:
- Data file deleted manually, `.meta.json` remains
- Cleanup scans for orphans and removes them
- Harmless but wastes disk space if not cleaned

**Corrupted Downloads**:
- If download interrupted, `.tmp` file remains
- Next cleanup removes any `.tmp` files older than 1 hour
- Hash verification ensures corrupt files aren't stored

**Cache Directory Migration**:
- If cache directory moves, all `Attachment::File` paths break
- Consider storing relative paths: `attachments/{hash}.{ext}`
- Resolve relative to `~/.local/share/rite/` at read time

**MIME Type Misdetection**:
- `infer` crate has comprehensive magic number database
- Fallback to `application/octet-stream` if unknown
- Telegram will accept any file as document

## Metrics & Observability

### Logging

```rust
// In publish_message
info!("Sending {} attachment(s) to Telegram: {}",
    msg.attachments.len(), names.join(", "));

// On error
error!("Failed to send attachment '{}': {}",
    attachment.name, err);
```

### Metrics to Track

- Number of attachments sent to Telegram
- Bytes uploaded to Telegram
- Download failures and retry counts
- Cache hit/miss ratio
- Average attachment size
- Cache cleanup frequency and freed bytes

## Summary

This design provides a simple, unified attachment system:

**Storage**:
- Content-addressed cache (`~/.local/share/rite/attachments/{hash}.{ext}`)
- Sidecar JSON metadata (`.meta.json`) for Telegram file_id, source info, MIME type
- Automatic deduplication (same content = same hash)
- Simple cleanup (age-based + size-based on `mtime`)

**Telegram Integration**:
- **Telegram → Bus**: Daemon downloads immediately, stores in cache, publishes message with `Attachment::File`
- **Bus → Telegram**: Read file, detect MIME (via `infer`), route to appropriate send method (photo/video/audio/document)
- **Inline attachments**: Format as code blocks or send as documents
- **URL attachments**: Embed as clickable links

**CLI Integration**:
- `rite send --attach <file>` copies to cache with same format
- Works identically to Telegram downloads
- Deduplication across all sources

**Benefits**:
- **Simple**: Flat directory, no complex queries, easy debugging
- **Unified**: Same storage for Telegram, CLI, agent APIs
- **Reliable**: Files cached locally, survive Telegram downtime
- **Secure**: MIME detection via magic numbers, not extensions
- **Efficient**: Deduplication, atomic writes, periodic cleanup

**Trade-offs**:
- Can't query "all files from project X" without scanning `.meta.json` files
- Cache cleanup is O(n) in number of files (acceptable for <1000 files)
- No transactions across multiple metadata files (not needed for single-daemon writer)
