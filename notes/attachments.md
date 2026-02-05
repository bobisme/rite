# BotBus ↔ Telegram Attachments Bridge Design

## Overview

This document outlines the design for bridging file attachments between BotBus messages and Telegram. Currently, BotBus supports three attachment types (File, Inline, Url) but the Telegram integration only displays attachment names as text. This design proposes a two-way attachment bridge that:

- Sends BotBus attachments to Telegram users (with fallback for unsupported types)
- Captures Telegram attachments (photos, documents, audio, video) and bridges them back to BotBus
- Handles storage, size limits, and format restrictions appropriately

## Current State

### BotBus Attachment System

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
BotBus file:src/config.rs
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

**File Type Detection**:
```
.rs, .py, .js, .ts, .go, .rs → Format as code block
.jpg, .png, .gif, .webp → sendPhoto
.mp4, .webm, .mkv → sendVideo
.mp3, .wav, .flac → sendAudio
.pdf, .txt, .md, .log → sendDocument
(other binary) → sendDocument
```

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

**New behavior**: Extract media from Telegram updates and create BotBus attachments

### Supported Telegram Media → BotBus Attachment Mapping

| Telegram Type | BotBus Type | Strategy |
|---------------|-------------|----------|
| **Photo** | File | Download and store with unique name |
| **Document** | File | Download and store with original filename |
| **Audio** | File | Download and store with `.mp3` or original ext |
| **Video** | File | Download and store with `.mp4` or original ext |
| **Voice Note** | File | Download and store as `.ogg` |
| **Animation** (GIF) | File | Download and store as `.mp4` or `.gif` |
| **Text** | Inline | Already handled; create inline attachment if large |

### Implementation Flow

```
Telegram Update
  ↓
Extract media_type (photo, document, video, etc.)
  ↓
If text + media:
   Create message with text body + media attachment
  ↓
If text only:
   Create message (current behavior)
  ↓
If media only (no text):
   Create message with media attachment + generic text
```

### Storage Strategy

**Challenge**: Where to store downloaded Telegram files?

**Design Decision**: Centralized attachment cache

**Location**: `~/.local/share/botbus/attachments/`
- Global (not per-project) so files can be shared across projects
- Directory structure: `<project-slug>/<channel-name>/<message-id>-<filename>`
- Follows BotBus global storage pattern

**Lifecycle**:
1. Download from Telegram to cache
2. Store path in BotBus `Attachment::File { path: "..." }`
3. Path is relative to home directory or absolute
4. Cleanup: Remove files older than 7 days (configurable)

**Size Management**:
- Max cache size: 500 MB (or configurable)
- When exceeded: LRU eviction
- Per-attachment limit: 50 MB (Telegram max)

### File Naming Convention

```
Format: {project_slug}/{channel}/{message_id}-{timestamp}-{original_or_type}.{ext}

Example:
botbus/general/01ARZ3NDEKTSV4RRFFQ69G5FAV-20250204T123456Z-photo.jpg
botbus/security/01ARZ3NDEKTSV4RRFFQ69G5FAV-20250204T123456Z-config.pdf
```

**Benefits**:
- Timestamp allows cleanup by age
- Message ID links back to original Telegram message
- Original filename preserved when possible
- Prevents collisions

## Storage Considerations

### Attachment Storage Options

#### Option A: Reference-based (Recommended)

**Approach**: Store Telegram file_id in BotBus attachment metadata

```json
{
  "name": "photo.jpg",
  "type": "telegram_file",
  "file_id": "AgAD...",
  "file_unique_id": "AQADy..."
}
```

**Pros**:
- No local storage needed
- Can re-download any time during 1-hour window
- Automatic cleanup (Telegram deletes after expiry)

**Cons**:
- Requires Telegram API call to retrieve
- File_id only valid for bot that generated it
- 1-hour expiry for file download URLs

#### Option B: Cache-based (Hybrid - Recommended)

**Approach**: Download and cache with reference fallback

```json
{
  "name": "photo.jpg",
  "type": "cached_file",
  "path": "~/.local/share/botbus/attachments/botbus/general/abc123.jpg",
  "telegram_file_id": "AgAD...",
  "cached_at": "2025-02-04T12:34:56Z"
}
```

**Pros**:
- Fast access via local file
- Survives Telegram API downtime
- User can reference files offline
- File ID backup for recovery

**Cons**:
- Disk storage required
- Cleanup needed to avoid disk bloat
- More complex metadata

**Recommendation**: Use Option B for media (photos, video, audio) but Option A for quick text documents

### Path Representation

**Internal**: Absolute paths stored in attachment metadata
```
/home/user/.local/share/botbus/attachments/botbus/general/msg-001-photo.jpg
```

**Display**: Relative paths when in BotBus home tree
```
attachments/botbus/general/msg-001-photo.jpg
```

**In Messages**: Markdown links or just the filename
```
Attachment: photo.jpg
```

## Size Limits and Quotas

### Per-Attachment Limits

| Limit | Value | Reasoning |
|-------|-------|-----------|
| **Telegram upload max** | 50 MB | Telegram Bot API hard limit |
| **BotBus message display** | 4000 chars | Telegram message limit |
| **Inline content max** | 10 KB | BotBus storage efficiency |
| **File attachment max** | 50 MB | Same as Telegram |
| **Voice/Audio max** | 20 MB | Telegram voice limit |

### Message-Level Limits

- **Max attachments per message**: 10
- **Max total attachment size per message**: 100 MB
- **Max total attachment size in memory**: 512 MB (prevent OOM)

### Cache Limits

- **Cache directory max size**: 500 MB
- **Age-based cleanup**: Files older than 7 days
- **Cache flush**: Automatic when size > threshold

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

### Core Extension to `TelegramClient`

```rust
// In src/telegram/client.rs

impl TelegramClient {
    /// Download media from Telegram and return file data
    pub async fn download_file(&self, file_id: &str) -> Result<Vec<u8>> { }

    /// Send photo to Telegram
    pub async fn send_photo(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_or_url: PhotoSource,
        caption: Option<&str>,
    ) -> Result<()> { }

    /// Send document to Telegram
    pub async fn send_document(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_or_url: DocumentSource,
        caption: Option<&str>,
    ) -> Result<()> { }

    /// Send audio to Telegram
    pub async fn send_audio(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        file_or_url: AudioSource,
        caption: Option<&str>,
    ) -> Result<()> { }
}

enum PhotoSource {
    Path(PathBuf),
    Url(String),
    FileId(String),
}

enum DocumentSource {
    Path(PathBuf),
    Url(String),
    FileId(String),
}
```

### Message Formatting for Attachments

**Bus → Telegram message format**:

```
Format: "{agent}: {body}\n\nAttachments: {list}\n{details}"

Example:
alice: Check the deploy logs

Attachments: config.rs, deploy.log

📎 config.rs (2 KB, code)
📎 deploy.log (15 KB, text)
```

**Telegram → Bus attachment wrapper**:

```rust
pub struct TelegramAttachmentMeta {
    pub file_id: String,
    pub file_unique_id: String,
    pub mime_type: Option<String>,
    pub file_size: u64,
    pub original_filename: Option<String>,
    pub cached_at: Option<DateTime<Utc>>,
    pub cache_path: Option<PathBuf>,
}
```

### Extended `Message` struct

**Add new attachment type**:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentContent {
    // ... existing types ...

    /// Telegram media reference (file_id for re-download)
    TelegramMedia {
        file_id: String,
        file_unique_id: String,
        mime_type: Option<String>,
        /// Optional local cache path
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_path: Option<String>,
    },
}
```

### Attachment Labels

Use message labels to mark attachment source:

```rust
// Suggested labels
message.labels.push("telegram_media".to_string());  // Came from Telegram
message.labels.push("photo".to_string());           // Type hint
message.labels.push("cached".to_string());          // Has local cache
```

## Implementation Phases

### Phase 1: Foundation (2-3 days)

1. Extend `TelegramClient` with media download capability
2. Add new `TelegramMedia` attachment type to core message
3. Add cache directory management and cleanup
4. Write tests for size validation and path handling

**Deliverables**:
- `download_file()` method
- Cache management utilities
- Attachment validation

### Phase 2: Bus → Telegram (3-5 days)

1. Update `publish_message()` to handle file attachments
2. Add `send_photo()`, `send_document()`, `send_audio()` methods
3. Implement file type detection and routing
4. Add attachment metadata encoding in captions
5. Error handling for missing/unreadable files

**Deliverables**:
- Photos, documents, and audio sent to Telegram
- Inline content formatted with syntax highlighting
- URLs embedded as links
- Error messages for unsupported types

### Phase 3: Telegram → Bus (3-5 days)

1. Extend `Update` and `TelegramMessage` to include media fields
2. Update `handle_update()` to extract media from updates
3. Implement download and cache logic
4. Create `Attachment` objects with TelegramMedia type
5. Add tests for various media types

**Deliverables**:
- Photos, documents, audio from Telegram received in BotBus
- Files cached locally with automatic cleanup
- Attachment metadata preserved

### Phase 4: Polish & Testing (2-3 days)

1. End-to-end testing (send from Bus → Telegram → Bus)
2. Edge cases (very large files, network failures, concurrent downloads)
3. Performance optimization (concurrent downloads, caching strategy)
4. Documentation and examples

**Deliverables**:
- Integration tests
- Performance benchmarks
- User documentation

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

### With Current BotBus Features

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

This design provides a pragmatic, bidirectional attachment bridge:

- **Bus → Telegram**: Convert different attachment types to appropriate Telegram media
- **Telegram → Bus**: Download media and cache locally with Telegram file_id fallback
- **Storage**: Centralized cache with automatic cleanup and LRU eviction
- **Limits**: Respect Telegram and BotBus constraints
- **Reliability**: Fallback mechanisms for API failures and file access issues
- **Extensibility**: Room for future features like compression, scanning, archival

The hybrid cache+reference approach balances storage efficiency with reliability.
