# BotBus Specification

> A chat-oriented coordination system for AI coding agents, written in Rust.

**Version**: 0.1.0 (Draft)
**Status**: Pre-implementation

---

## Table of Contents

1. [Overview](#overview)
2. [Design Principles](#design-principles)
3. [Architecture](#architecture)
4. [Data Model](#data-model)
5. [Storage Layer](#storage-layer)
6. [CLI Interface](#cli-interface)
7. [TUI Interface](#tui-interface)
8. [File Claims](#file-claims)
9. [Concurrency & Synchronization](#concurrency--synchronization)
10. [Error Handling](#error-handling)
11. [Configuration](#configuration)
12. [Project Structure](#project-structure)
13. [Dependencies](#dependencies)
14. [Testing Strategy](#testing-strategy)
15. [Future Considerations](#future-considerations)

---

## Overview

### What is BotBus?

BotBus is a lightweight, CLI-first chat system designed for coordinating multiple AI coding agents working on the same project. Think of it as IRC or Slack for your coding agents.

### Why BotBus?

When multiple AI agents work on a codebase simultaneously, they need:

- **Communication**: Share intent, progress, and discoveries
- **Coordination**: Avoid editing the same files simultaneously
- **Visibility**: Humans and agents should see what's happening
- **Audit Trail**: Immutable record of all agent communication

### How is it Different from MCP Agent Mail?

| Aspect | MCP Agent Mail | BotBus |
|--------|----------------|--------|
| Mental model | Email (inbox/outbox) | Chat (channels/streams) |
| Primary interface | MCP server (tools) | CLI commands |
| Implementation | Python + FastMCP | Rust |
| Threading | Explicit thread IDs | Channels are threads |
| UI | Web browser | Terminal TUI |
| Storage | Git + SQLite | Append-only logs + SQLite index |

---

## Design Principles

### 1. CLI-First

The primary interface is shell commands. Agents invoke `botbus send`, `botbus watch`, etc. The TUI is for monitoring, not the primary interaction mode.

### 2. Append-Only Logs as Source of Truth

All messages are stored in JSONL (JSON Lines) files. These logs are:
- Human-readable
- Git-friendly (append-only = clean diffs)
- Trivially parseable
- The canonical source of truth

### 3. Derived Indexes

SQLite is used only for full-text search indexing. The index can always be rebuilt from the JSONL logs. If the index is corrupted or deleted, no data is lost.

### 4. Per-Project Isolation

Each git repository/project has its own `.botbus/` directory with isolated channels, agents, and claims. No cross-project state by default.

### 5. Immutable Messages

Messages cannot be edited or deleted. This provides a complete audit trail of agent communication and decision-making.

### 6. No Daemon Required

BotBus operates without a persistent background process. File watching (inotify/FSEvents/etc.) enables real-time updates without a daemon.

### 7. Cross-Platform

Must work on Linux, macOS, and Windows.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         CLI Layer                           │
│  botbus send, botbus watch, botbus history, botbus ui       │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                      Core Library                           │
│  - Project management (init, detect)                        │
│  - Agent identity                                           │
│  - Channel operations                                       │
│  - Message read/write                                       │
│  - File claims                                              │
│  - Index synchronization                                    │
└──────────┬──────────────────────────────┬───────────────────┘
           │                              │
           ▼                              ▼
┌─────────────────────┐      ┌────────────────────────────────┐
│    Storage Layer    │      │         Index Layer            │
│  - JSONL append     │      │  - SQLite FTS5                 │
│  - File locking     │      │  - Incremental sync            │
│  - File watching    │      │  - Rebuild from logs           │
└─────────────────────┘      └────────────────────────────────┘
           │                              │
           ▼                              ▼
┌─────────────────────┐      ┌────────────────────────────────┐
│   .botbus/channels/ │      │    .botbus/index.sqlite        │
│   .botbus/agents.jsonl     │                                │
│   .botbus/claims.jsonl     │                                │
│   .botbus/state.json│      │                                │
└─────────────────────┘      └────────────────────────────────┘
```

### Component Responsibilities

**CLI Layer**
- Parse command-line arguments (clap)
- Format output for terminal
- Handle user input
- Launch TUI when requested

**Core Library**
- Business logic for all operations
- No I/O concerns (uses traits for storage abstraction)
- Testable in isolation

**Storage Layer**
- File I/O with proper locking
- JSONL serialization
- File watching for real-time updates
- Cross-platform file operations

**Index Layer**
- SQLite FTS5 for full-text search
- Incremental synchronization from logs
- Query interface for search operations

---

## Data Model

### Message

The fundamental unit of communication.

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Timestamp when the message was created
    pub ts: DateTime<Utc>,
    
    /// Unique identifier (ULID for sortability without coordination)
    pub id: Ulid,
    
    /// Name of the sending agent
    pub agent: String,
    
    /// Channel name, or "_dm_{agent1}_{agent2}" for DMs (names sorted)
    pub channel: String,
    
    /// Message content (markdown supported)
    pub body: String,
    
    /// Extracted @mentions for potential notifications
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
    
    /// Optional structured metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<MessageMeta>,
}
```

### MessageMeta

Structured metadata for special message types.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageMeta {
    /// Agent claimed files for editing
    Claim {
        patterns: Vec<String>,
        ttl_secs: u64,
    },
    
    /// Agent released file claims
    Release {
        patterns: Vec<String>,
    },
    
    /// System event (agent joined, etc.)
    System {
        event: SystemEvent,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemEvent {
    AgentRegistered,
    AgentRenamed { old_name: String },
    ClaimExpired { patterns: Vec<String> },
}
```

### Agent

Registered agent identity within a project.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Timestamp of registration
    pub ts: DateTime<Utc>,
    
    /// Unique agent name within this project
    pub name: String,
    
    /// Optional description or identifier (e.g., "Claude Sonnet 3.5")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    
    /// Registration event type
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    Registered,
    Renamed { old_name: String },
}
```

### FileClaim

A claim on files/patterns for editing.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileClaim {
    /// Timestamp when claim was created
    pub ts: DateTime<Utc>,
    
    /// Unique identifier
    pub id: Ulid,
    
    /// Agent that owns this claim
    pub agent: String,
    
    /// Glob patterns being claimed (e.g., "src/auth/**/*.rs")
    pub patterns: Vec<String>,
    
    /// When the claim expires (UTC)
    pub expires_at: DateTime<Utc>,
    
    /// Whether this claim is still active
    pub active: bool,
    
    /// Event type (created, released, expired)
    pub event: ClaimEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimEvent {
    Created,
    Released,
    Expired,
}
```

### Channel

Channels are implicit - they exist if there are messages in them. No explicit channel object is stored. Channel metadata (if needed later) would be derived from message history.

**Channel Naming Rules:**
- Regular channels: lowercase alphanumeric + hyphens, 1-64 chars
- DM channels: `_dm_{agent1}_{agent2}` where names are sorted alphabetically
- Reserved prefix: `_` for system channels

---

## Storage Layer

### Directory Structure

```
project-root/
└── .botbus/
    ├── channels/
    │   ├── general.jsonl      # Default channel
    │   ├── backend.jsonl
    │   ├── frontend.jsonl
    │   └── _dm_alice_bob.jsonl
    ├── agents.jsonl           # Agent registrations
    ├── claims.jsonl           # File claims
    ├── state.json             # Mutable state (cursors, etc.)
    └── index.sqlite           # FTS index (derived)
```

### File Formats

**JSONL Files** (channels/*.jsonl, agents.jsonl, claims.jsonl)
- One JSON object per line
- UTF-8 encoded
- Lines terminated with `\n` (Unix-style)
- Append-only (no modifications to existing lines)

**state.json**
- Mutable JSON file
- Contains read cursors, last-seen timestamps, etc.
- Small, infrequently written
- Can be regenerated if lost

**index.sqlite**
- SQLite database with FTS5 virtual tables
- Derived from JSONL logs
- Can be deleted and rebuilt

### File Locking Strategy

To prevent corruption from concurrent writes:

```rust
use fs2::FileExt;
use std::fs::OpenOptions;

fn append_line(path: &Path, line: &str) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    
    // Acquire exclusive lock (blocks until available)
    file.lock_exclusive()?;
    
    // Write the line
    writeln!(&file, "{}", line)?;
    
    // Flush to ensure data is written
    file.sync_all()?;
    
    // Lock is released when file is dropped
    Ok(())
}
```

**Lock Scope:**
- Lock is per-file, not per-directory
- Locks are advisory (cooperative)
- Locks are released on process termination

### File Watching

For real-time updates without polling:

```rust
use notify::{Watcher, RecursiveMode, Event};
use std::sync::mpsc;
use std::time::Duration;

fn watch_channels(botbus_dir: &Path) -> Result<mpsc::Receiver<Event>> {
    let (tx, rx) = mpsc::channel();
    
    let mut watcher = notify::recommended_watcher(move |res| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })?;
    
    watcher.watch(
        &botbus_dir.join("channels"),
        RecursiveMode::Recursive
    )?;
    
    Ok(rx)
}
```

**Debouncing:**
- Multiple rapid writes may trigger multiple events
- Consumer should debounce (e.g., 100ms window)
- Read from last-known position, not entire file

---

## CLI Interface

### Global Options

```
botbus [OPTIONS] <COMMAND>

Options:
    -p, --project <PATH>    Project directory (default: auto-detect)
    -q, --quiet             Suppress non-essential output
    -v, --verbose           Increase verbosity
    --json                  Output in JSON format (where applicable)
    -h, --help              Print help
    -V, --version           Print version
```

### Commands

#### `botbus init`

Initialize BotBus in a project directory.

```
botbus init [OPTIONS]

Options:
    --force    Overwrite existing .botbus directory
```

**Behavior:**
1. Create `.botbus/` directory
2. Create `channels/` subdirectory
3. Create empty `agents.jsonl`, `claims.jsonl`
4. Create default `state.json`
5. Add `.botbus/index.sqlite` to `.gitignore` if git repo detected

#### `botbus register`

Register an agent identity in the current project.

```
botbus register [OPTIONS]

Options:
    -n, --name <NAME>         Agent name (auto-generated if omitted)
    -d, --description <DESC>  Optional description
```

**Behavior:**
1. If `--name` not provided, generate random adjective+noun name
2. Check name doesn't already exist in `agents.jsonl`
3. Append registration record to `agents.jsonl`
4. Store identity in `state.json` for subsequent commands
5. Post system message to `#general`: "AgentName has joined"

**Auto-generated Names:**
Format: `{Adjective}{Noun}` (PascalCase)
- Adjectives: Blue, Green, Red, Swift, Brave, Calm, Wild, etc. (50+)
- Nouns: Castle, Forest, River, Mountain, Lake, Storm, etc. (50+)
- If collision, append 2-digit number

#### `botbus whoami`

Display current agent identity.

```
botbus whoami
```

**Output:**
```
Agent: BlueCastle
Project: /home/user/myproject
Registered: 2026-01-23T15:30:00Z
```

#### `botbus send`

Send a message to a channel or agent.

```
botbus send <TARGET> <MESSAGE>

Arguments:
    <TARGET>     Channel name or @agent for DM
    <MESSAGE>    Message content (use quotes for multi-word)

Options:
    --meta <JSON>    Attach metadata (advanced)
```

**Examples:**
```bash
botbus send general "Starting work on auth module"
botbus send @GreenForest "Can you review my changes?"
botbus send backend "Found a bug in the middleware"
```

**Behavior:**
1. Validate agent is registered
2. If target starts with `@`, create DM channel name
3. Create channel file if it doesn't exist
4. Extract @mentions from message body
5. Append message to channel JSONL
6. Update FTS index (async/background)

#### `botbus history`

View message history.

```
botbus history [CHANNEL] [OPTIONS]

Arguments:
    [CHANNEL]    Channel to view (default: general)

Options:
    -n, --count <N>      Number of messages (default: 50)
    -f, --follow         Follow mode (like tail -f)
    --since <DATETIME>   Messages after this time
    --before <DATETIME>  Messages before this time
    --from <AGENT>       Filter by sender
```

**Output Format:**
```
#general
[10:23] BlueCastle: Starting work on auth module
[10:24] GreenForest: I'll take the API routes
[10:25] BlueCastle: Sounds good, I've claimed src/auth/**
```

#### `botbus watch`

Stream new messages in real-time.

```
botbus watch [CHANNEL] [OPTIONS]

Arguments:
    [CHANNEL]    Channel to watch (default: all)

Options:
    --all        Watch all channels
```

**Behavior:**
1. Print last 10 messages for context
2. Set up file watcher on channel(s)
3. Print new messages as they arrive
4. Continue until Ctrl+C

#### `botbus channels`

List all channels.

```
botbus channels [OPTIONS]

Options:
    --all        Include DM channels
```

**Output:**
```
Channels:
  #general      12 messages, last: 5m ago
  #backend       8 messages, last: 2h ago
  #frontend      3 messages, last: 1d ago
```

#### `botbus agents`

List registered agents.

```
botbus agents [OPTIONS]

Options:
    --active     Only show recently active agents
```

**Output:**
```
Agents:
  BlueCastle     Registered 2h ago, last seen 5m ago
  GreenForest    Registered 1h ago, last seen 10m ago
  RedMountain    Registered 3h ago, last seen 2h ago
```

#### `botbus search`

Full-text search messages.

```
botbus search <QUERY> [OPTIONS]

Arguments:
    <QUERY>    Search query (supports FTS5 syntax)

Options:
    -c, --channel <CHANNEL>    Limit to channel
    -n, --count <N>            Max results (default: 20)
    --from <AGENT>             Filter by sender
```

**Examples:**
```bash
botbus search "authentication bug"
botbus search "api OR endpoint" -c backend
botbus search "error" --from BlueCastle
```

#### `botbus claim`

Claim files for editing (advisory lock).

```
botbus claim <PATTERN>... [OPTIONS]

Arguments:
    <PATTERN>...    Glob patterns to claim

Options:
    -t, --ttl <SECONDS>    Time-to-live (default: 3600)
    -m, --message <MSG>    Optional message about the claim
```

**Examples:**
```bash
botbus claim "src/auth/**/*.rs"
botbus claim "src/api/routes.rs" "src/api/handlers.rs" -t 7200
botbus claim "*.toml" -m "Updating dependencies"
```

**Behavior:**
1. Check for conflicts with existing active claims
2. If conflict, print warning but still create claim (advisory)
3. Append claim to `claims.jsonl`
4. Post message to `#general` about the claim

#### `botbus claims`

List active file claims.

```
botbus claims [OPTIONS]

Options:
    --all        Include expired claims
    --mine       Only show my claims
```

**Output:**
```
Active Claims:
  BlueCastle    src/auth/**/*.rs       expires in 45m
  GreenForest   src/api/routes.rs      expires in 1h 20m

Conflicts:
  (none)
```

#### `botbus release`

Release file claims.

```
botbus release [PATTERN]... [OPTIONS]

Arguments:
    [PATTERN]...    Patterns to release (default: all your claims)

Options:
    --all    Release all your claims
```

#### `botbus ui`

Launch the terminal UI.

```
botbus ui [OPTIONS]

Options:
    -c, --channel <CHANNEL>    Start in this channel
```

---

## TUI Interface

### Layout

```
┌─ BotBus ─────────────────────────────────────────────────────┐
│ Channels     │ #general                                      │
│ ──────────── │ ──────────────────────────────────────────── │
│ > #general   │ [10:23] BlueCastle: Starting work on auth    │
│   #backend   │ [10:24] GreenForest: I'll take the API routes│
│   #frontend  │ [10:25] BlueCastle: I've claimed src/auth/** │
│   @BlueCastle│ [10:31] GreenForest: Found a bug in middleware│
│              │ [10:32] BlueCastle: Can you elaborate?        │
│              │ ──────────────────────────────────────────── │
│ Agents       │ > █                                           │
│ ──────────── │                                               │
│ ● BlueCastle │                                               │
│ ● GreenForest│                                               │
│ ○ RedMountain│                                               │
├──────────────┴───────────────────────────────────────────────┤
│ [Tab] pane  [/] search  [c] claims  [q] quit  [Enter] send  │
└──────────────────────────────────────────────────────────────┘
```

### Panes

1. **Channel List** (left, top)
   - Shows all channels
   - Current channel highlighted
   - Unread indicator (if applicable)

2. **Agent List** (left, bottom)
   - Shows registered agents
   - Activity indicator (●/○)

3. **Message Area** (right, main)
   - Scrollable message history
   - New messages appear at bottom
   - Timestamps in local time

4. **Input Area** (bottom of message area)
   - Text input for composing messages
   - Tab-completion for @mentions and channels

5. **Status Bar** (bottom)
   - Keyboard shortcuts
   - Current mode indicator

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Tab` | Switch between panes |
| `j/k` or `↑/↓` | Navigate lists / scroll messages |
| `Enter` | Select channel / send message |
| `/` | Open search |
| `c` | Show claims overlay |
| `n` | New DM (prompt for agent) |
| `Esc` | Close overlay / cancel |
| `q` | Quit |
| `?` | Help overlay |

### Real-Time Updates

The TUI uses file watching to update in real-time:
1. Watch `.botbus/channels/` for changes
2. On change event, read new messages from affected file
3. Update message list and scroll to bottom (if already at bottom)
4. Update unread indicators in channel list

---

## File Claims

### Purpose

File claims are **advisory locks** that signal an agent's intent to edit certain files. They help prevent conflicts but don't enforce exclusivity.

### Claim Lifecycle

```
Created ──────────────────────────────────────────> Expired
    │                                                  │
    │ (agent calls `botbus release`)                   │
    │                                                  │
    └──────────────> Released <────────────────────────┘
```

### Conflict Detection

When an agent attempts to claim files:

```rust
fn check_conflicts(new_patterns: &[String], claims: &[FileClaim]) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    
    for claim in claims.iter().filter(|c| c.active && c.expires_at > Utc::now()) {
        for new_pat in new_patterns {
            for existing_pat in &claim.patterns {
                if patterns_overlap(new_pat, existing_pat) {
                    conflicts.push(Conflict {
                        your_pattern: new_pat.clone(),
                        existing_pattern: existing_pat.clone(),
                        holder: claim.agent.clone(),
                        expires_at: claim.expires_at,
                    });
                }
            }
        }
    }
    
    conflicts
}
```

### Pattern Matching

Uses glob patterns compatible with `.gitignore`:
- `*` matches any sequence except `/`
- `**` matches any sequence including `/`
- `?` matches any single character
- `[abc]` matches any character in brackets

### Automatic Expiration

Claims expire after their TTL. Expiration is checked:
1. On any claim-related command
2. In the TUI claims view
3. Background cleanup is NOT automatic (no daemon)

Expired claims are marked with `event: expired` in the log.

---

## Concurrency & Synchronization

### Multi-Process Safety

Multiple processes (agents) may operate simultaneously:

1. **JSONL Append**: Protected by file-level exclusive locks
2. **State.json Updates**: Protected by file-level exclusive locks
3. **SQLite Index**: SQLite handles its own locking (WAL mode)
4. **Reads**: No locking required (append-only files are safe to read)

### Race Conditions

**Scenario**: Two agents send messages at the same time

```
Agent A                    Agent B
   │                          │
   ├── lock(general.jsonl)    │
   │                          ├── lock(general.jsonl) [blocks]
   ├── append message         │
   ├── unlock                 │
   │                          ├── [acquires lock]
   │                          ├── append message
   │                          └── unlock
```

Messages are serialized correctly. Order depends on who acquires lock first.

**Scenario**: Agent reads while another writes

```
Agent A (writing)           Agent B (reading)
   │                          │
   ├── lock                   │
   ├── append "line N"        │
   ├── fsync                  │
   ├── unlock                 │
   │                          ├── read file
   │                          │   (sees complete line N)
```

Because we `fsync` before unlocking, readers always see complete lines.

### Index Synchronization

The FTS index is updated incrementally:

```rust
fn sync_index(channel: &str, last_offset: u64) -> Result<u64> {
    let file = File::open(channel_path(channel))?;
    file.seek(SeekFrom::Start(last_offset))?;
    
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let msg: Message = serde_json::from_str(&line?)?;
        insert_into_fts(&msg)?;
    }
    
    Ok(file.stream_position()?)
}
```

Offsets are stored in `state.json` per channel.

---

## Error Handling

### Error Types

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum BotBusError {
    #[error("Not in a BotBus project. Run 'botbus init' first.")]
    NotInitialized,
    
    #[error("Agent not registered. Run 'botbus register' first.")]
    NotRegistered,
    
    #[error("Agent name '{0}' is already taken")]
    AgentNameTaken(String),
    
    #[error("Channel '{0}' not found")]
    ChannelNotFound(String),
    
    #[error("Invalid channel name: {0}")]
    InvalidChannelName(String),
    
    #[error("File claim conflict with {agent}: {pattern}")]
    ClaimConflict { agent: String, pattern: String },
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    
    #[error("Lock acquisition timed out")]
    LockTimeout,
}
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Invalid arguments |
| 3 | Not initialized |
| 4 | Not registered |
| 5 | Conflict (claims) |

### User-Friendly Messages

All errors should:
1. Explain what went wrong
2. Suggest how to fix it
3. Use color (red for errors, yellow for warnings)

```
Error: Not in a BotBus project.

  Run 'botbus init' to initialize BotBus in this directory,
  or use '--project <PATH>' to specify a different location.
```

---

## Configuration

### Global Configuration

Location: `~/.config/botbus/config.toml` (XDG) or `~/.botbus/config.toml`

```toml
# Default agent name (used if not registered in project)
default_agent = "BlueCastle"

# Default claim TTL in seconds
default_claim_ttl = 3600

# TUI settings
[tui]
# Color theme: "dark" or "light"
theme = "dark"
# Timestamp format
timestamp_format = "%H:%M"
# Show agent colors
agent_colors = true
```

### Project Configuration

Location: `.botbus/config.toml` (optional)

```toml
# Project-specific overrides
[project]
name = "MyProject"

# Channels created automatically on init
default_channels = ["general", "backend", "frontend"]

# Claim settings
[claims]
default_ttl = 7200
warn_on_conflict = true
```

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `BOTBUS_PROJECT` | Override project directory |
| `BOTBUS_AGENT` | Override agent name |
| `BOTBUS_NO_COLOR` | Disable colored output |
| `BOTBUS_DEBUG` | Enable debug logging |

---

## Project Structure

```
botbus/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── SPEC.md                 # This document
├── LICENSE
├── .gitignore
├── src/
│   ├── main.rs             # Entry point, CLI setup
│   ├── lib.rs              # Library root, re-exports
│   ├── cli/
│   │   ├── mod.rs          # CLI module root
│   │   ├── init.rs         # botbus init
│   │   ├── register.rs     # botbus register
│   │   ├── send.rs         # botbus send
│   │   ├── history.rs      # botbus history
│   │   ├── watch.rs        # botbus watch
│   │   ├── search.rs       # botbus search
│   │   ├── channels.rs     # botbus channels
│   │   ├── agents.rs       # botbus agents
│   │   ├── claim.rs        # botbus claim, release, claims
│   │   └── ui.rs           # botbus ui (launches TUI)
│   ├── core/
│   │   ├── mod.rs          # Core module root
│   │   ├── project.rs      # Project detection, init
│   │   ├── agent.rs        # Agent identity, registration
│   │   ├── channel.rs      # Channel operations
│   │   ├── message.rs      # Message struct, creation
│   │   ├── claim.rs        # File claims logic
│   │   └── names.rs        # Name generation
│   ├── storage/
│   │   ├── mod.rs          # Storage module root
│   │   ├── jsonl.rs        # JSONL read/write with locking
│   │   ├── state.rs        # state.json management
│   │   └── watch.rs        # File watching
│   ├── index/
│   │   ├── mod.rs          # Index module root
│   │   ├── schema.rs       # SQLite schema
│   │   ├── fts.rs          # FTS operations
│   │   └── sync.rs         # Log -> index synchronization
│   └── tui/
│       ├── mod.rs          # TUI module root
│       ├── app.rs          # Application state
│       ├── ui.rs           # Layout and rendering
│       ├── events.rs       # Input handling
│       ├── widgets/
│       │   ├── mod.rs
│       │   ├── channel_list.rs
│       │   ├── agent_list.rs
│       │   ├── message_area.rs
│       │   └── input.rs
│       └── theme.rs        # Colors and styles
└── tests/
    ├── integration/
    │   ├── cli_tests.rs    # CLI integration tests
    │   └── tui_tests.rs    # TUI integration tests
    └── unit/
        ├── message_tests.rs
        ├── claim_tests.rs
        └── storage_tests.rs
```

---

## Dependencies

### Cargo.toml

```toml
[package]
name = "botbus"
version = "0.1.0"
edition = "2021"
authors = ["Your Name"]
description = "Chat-oriented coordination for AI coding agents"
license = "MIT"
repository = "https://github.com/yourname/botbus"

[dependencies]
# CLI framework
clap = { version = "4", features = ["derive", "env"] }

# Async runtime
tokio = { version = "1", features = ["full"] }

# TUI
ratatui = "0.28"
crossterm = "0.28"

# File watching
notify = "6"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Database
rusqlite = { version = "0.32", features = ["bundled", "fts5"] }

# Time
chrono = { version = "0.4", features = ["serde"] }

# Unique IDs
ulid = { version = "1", features = ["serde"] }

# File locking
fs2 = "0.4"

# Glob patterns
globset = "0.4"

# Terminal colors
colored = "2"

# Path utilities
directories = "5"

# Error handling
anyhow = "1"
thiserror = "2"

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

### Dependency Rationale

| Crate | Purpose | Why This One |
|-------|---------|--------------|
| `clap` | CLI parsing | Best-in-class, derive macros |
| `tokio` | Async runtime | De facto standard, needed for file watching |
| `ratatui` | TUI framework | Active community fork of tui-rs |
| `crossterm` | Terminal backend | Cross-platform, pure Rust |
| `notify` | File watching | Cross-platform, async support |
| `serde` | Serialization | Industry standard |
| `rusqlite` | SQLite | Mature, FTS5 support |
| `chrono` | Time handling | Full-featured, serde support |
| `ulid` | Unique IDs | Sortable, no coordination needed |
| `fs2` | File locking | Cross-platform flock |
| `globset` | Glob matching | Fast, gitignore-compatible |

---

## Testing Strategy

### Unit Tests

Located in `tests/unit/` and inline with `#[cfg(test)]`.

**Coverage targets:**
- Message serialization/deserialization
- Claim conflict detection
- Name generation
- Channel name validation
- Pattern matching

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_message_roundtrip() {
        let msg = Message {
            ts: Utc::now(),
            id: Ulid::new(),
            agent: "TestAgent".into(),
            channel: "general".into(),
            body: "Hello, world!".into(),
            mentions: vec![],
            meta: None,
        };
        
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Message = serde_json::from_str(&json).unwrap();
        
        assert_eq!(msg.id, parsed.id);
        assert_eq!(msg.body, parsed.body);
    }
    
    #[test]
    fn test_claim_conflict_detection() {
        let existing = FileClaim {
            patterns: vec!["src/auth/**".into()],
            agent: "Agent1".into(),
            // ...
        };
        
        assert!(patterns_overlap("src/auth/login.rs", "src/auth/**"));
        assert!(!patterns_overlap("src/api/routes.rs", "src/auth/**"));
    }
}
```

### Integration Tests

Located in `tests/integration/`.

**Test scenarios:**
- Full CLI workflow (init → register → send → history)
- Multiple agents interacting
- Concurrent writes
- File watching updates
- Search functionality

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn test_init_creates_directory() {
    let temp = TempDir::new().unwrap();
    
    Command::cargo_bin("botbus")
        .unwrap()
        .current_dir(&temp)
        .arg("init")
        .assert()
        .success();
    
    assert!(temp.path().join(".botbus").exists());
    assert!(temp.path().join(".botbus/channels").exists());
}

#[test]
fn test_send_receive_flow() {
    let temp = TempDir::new().unwrap();
    
    // Init
    Command::cargo_bin("botbus").unwrap()
        .current_dir(&temp)
        .args(["init"])
        .assert().success();
    
    // Register
    Command::cargo_bin("botbus").unwrap()
        .current_dir(&temp)
        .args(["register", "--name", "TestAgent"])
        .assert().success();
    
    // Send
    Command::cargo_bin("botbus").unwrap()
        .current_dir(&temp)
        .args(["send", "general", "Hello, world!"])
        .assert().success();
    
    // History
    Command::cargo_bin("botbus").unwrap()
        .current_dir(&temp)
        .args(["history", "general"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello, world!"));
}
```

### TUI Tests

TUI testing is challenging. Options:
1. **Snapshot testing**: Render to buffer, compare with expected output
2. **Component testing**: Test individual widgets in isolation
3. **Manual testing**: Some TUI behavior requires manual verification

---

## Future Considerations

### Potential Features (Post-MVP)

1. **MCP Server Mode**
   - Expose BotBus functionality as MCP tools
   - Allow integration with MCP-native agents

2. **Webhooks / Notifications**
   - HTTP webhook on new messages
   - Desktop notifications

3. **Message Reactions**
   - Emoji reactions to messages
   - Useful for lightweight acknowledgment

4. **Message References**
   - Reply to specific messages
   - Quote messages

5. **Slash Commands**
   - `/claim`, `/release` in message body
   - `/status` to show agent status

6. **Cross-Project Channels**
   - Link channels across projects
   - Useful for monorepo or related projects

7. **Message Expiry**
   - Optional TTL for ephemeral channels
   - Auto-cleanup of old messages

8. **Encryption**
   - End-to-end encryption for DMs
   - Encrypted channels

9. **Git Integration**
   - Pre-commit hook for claim warnings
   - Auto-release claims on commit

10. **Web UI**
    - Read-only web view of channels
    - Useful for remote monitoring

### Non-Goals

- **Real-time sync across machines**: This is a local tool
- **User authentication**: Trust is assumed within a project
- **Message editing**: Immutability is a feature
- **Rich media**: Text-only (code blocks, markdown)
- **Plugins/extensions**: Keep it simple

---

## Appendix: Name Word Lists

### Adjectives (sample)

```
Blue, Green, Red, Gold, Silver, Bronze, Amber, Jade,
Swift, Brave, Calm, Wild, Bold, Keen, Wise, True,
Silent, Gentle, Fierce, Noble, Ancient, Cosmic, Crystal, Digital,
Electric, Frozen, Golden, Hidden, Iron, Jasper, Lunar, Mystic,
Northern, Onyx, Primal, Quantum, Radiant, Sacred, Thunder, Ultra,
Velvet, Wandering, Xenon, Yielding, Zealous
```

### Nouns (sample)

```
Castle, Forest, River, Mountain, Lake, Storm, Eagle, Wolf,
Phoenix, Dragon, Falcon, Hawk, Raven, Tiger, Lion, Bear,
Anchor, Beacon, Circuit, Depot, Engine, Forge, Gateway, Harbor,
Index, Junction, Kernel, Lattice, Matrix, Nexus, Oracle, Portal,
Quartz, Relay, Sentinel, Tower, Umbra, Vertex, Warden, Zenith
```

---

## Revision History

| Version | Date | Changes |
|---------|------|---------|
| 0.1.0 | 2026-01-23 | Initial draft |
