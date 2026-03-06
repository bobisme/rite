# Rite Git Sync Design

## Overview

Enable multi-machine sync of Rite data directory using git with GitHub (free tier). Rite already uses event-sourced append-only JSONL files, making git sync trivial with union merge.

**Key insight**: Rite architecture is already perfect for git sync:
- Messages use ULIDs (globally unique, time-sortable)
- Claims/agents use event sourcing (created/released events)
- SQLite is just a projection (can rebuild from JSONL)
- No merge conflicts possible with union merge

## Architecture

### Data Model (Already Implemented)

**JSONL Event Logs** (source of truth, git-tracked):
```
~/.local/share/rite/
├── messages/
│   ├── general.jsonl       # Messages with ULIDs
│   ├── team.jsonl
│   └── @agent-name.jsonl   # DMs
├── claims.jsonl            # Claim events: created/released
├── agents.jsonl            # Agent registration events
└── channels.jsonl          # Channel metadata
```

**SQLite Index** (derived, gitignored):
```
├── index.db               # FTS search index
├── index.db-wal
└── index.db-shm
```

**Local State** (machine-local, gitignored):
```
└── state.json            # Read cursors, offsets, local prefs
```

**Attachments** (handled separately, future work):
```
└── attachments/
    ├── {hash}.{ext}
    └── {hash}.{ext}.meta.json
```

### Git Configuration

**`.gitattributes`** (auto-created by `rite sync init`):
```gitattributes
# Union merge for append-only JSONL
*.jsonl merge=union

# Binary files (don't merge)
*.db binary
*.db-wal binary
*.db-shm binary

# Attachments (future: git-annex or reference-only)
# For now: just ignore
attachments/** binary
```

**`.gitignore`** (auto-created by `rite sync init`):
```gitignore
# SQLite indexes (derived from JSONL)
*.db
*.db-wal
*.db-shm

# Local state (machine-specific)
state.json

# Attachments (synced separately, or reference-only)
attachments/

# Temp files
*.tmp
*.lock
```

## Union Merge Strategy

**How union merge works**:
- Keeps all lines from both sides of a merge
- Appends "ours" and "theirs" together
- Perfect for append-only JSONL

**Example**:
```
# Machine A's general.jsonl
{"id":"01HQ2AAA","body":"from A"}
{"id":"01HQ2BBB","body":"another A"}

# Machine B's general.jsonl
{"id":"01HQ2AAA","body":"from A"}      # Already synced
{"id":"01HQ2CCC","body":"from B"}

# After union merge
{"id":"01HQ2AAA","body":"from A"}
{"id":"01HQ2BBB","body":"another A"}
{"id":"01HQ2AAA","body":"from A"}      # Duplicate
{"id":"01HQ2CCC","body":"from B"}

# Rite reads and dedupes
# → [01HQ2AAA, 01HQ2BBB, 01HQ2CCC]
```

**Why this works**:
- ULIDs are globally unique (no real duplicates)
- Rite dedupes by ULID when reading
- SQLite index rebuilt after merge (deduped)

## Workflow

### Setup (One-Time)

```bash
# Initialize git repo in data dir
rite sync init --remote git@github.com:user/rite-data.git
# → cd ~/.local/share/rite
# → git init
# → Creates .gitattributes (union merge)
# → Creates .gitignore (*.db, state.json, attachments/)
# → git add .gitattributes .gitignore
# → git commit -m "chore: initialize rite data repo"
# → git remote add origin <remote>
# → git push -u origin main

# Or: init without remote (local-only)
rite sync init
# → Same but no remote configured
```

### Auto-Commit (Transparent)

**After every Rite operation**:
```bash
rite send general "message"
# Internally:
# 1. Appends to general.jsonl
# 2. Updates SQLite index
# 3. git add messages/general.jsonl
# 4. git commit -m "add message to #general"
#    (local commit only, no push)
```

**Operations that trigger auto-commit**:
- `rite send` - commit message JSONL
- `rite claim` - commit claims.jsonl
- `rite release` - commit claims.jsonl
- Agent registration - commit agents.jsonl
- Channel operations - commit channels.jsonl

**Commit messages**:
- `"add message to #{channel}"`
- `"claim {patterns}"`
- `"release claim {id}"`
- `"register agent {name}"`

### Manual Sync

```bash
# Push local changes
rite sync
# or: rite sync --push
# → git push origin main

# Pull remote changes
rite sync --pull
# → git fetch origin
# → git merge origin/main
#    (union merge handles JSONL automatically)
# → rite index rebuild --if-needed
#    (rebuild SQLite if JSONL changed)

# Pull and push
rite sync --pull --push
# → Pull first, then push
```

### Periodic Sync (Optional)

```bash
# In cron: sync every hour
0 * * * * cd ~/.local/share/rite && rite sync --pull --push

# Or: systemd timer
# ~/.config/systemd/user/rite-sync.timer
[Unit]
Description=Rite sync timer

[Timer]
OnCalendar=hourly
Persistent=true

[Install]
WantedBy=timers.target
```

## SQLite Index Rebuild

### When to Rebuild

**After git pull/merge**:
- Check if any `*.jsonl` files changed
- If changed: rebuild index

**Strategies**:
1. **Eager**: Rebuild immediately after merge
2. **Lazy**: Rebuild on first query if JSONL newer than index
3. **Hybrid**: Rebuild in background after merge

**Recommendation**: Hybrid (non-blocking)

### Implementation

```bash
rite index rebuild
# → Drops all SQLite FTS tables
# → Reads all messages from JSONL
# → Dedupes by ULID
# → Sorts by ULID (chronological)
# → Rebuilds FTS index
# → Updates sync_state table

# Check if rebuild needed
rite index status
# → Compares JSONL mtime vs index mtime
# → Shows: "Index up to date" or "Needs rebuild (3 files changed)"

# Auto-rebuild (called internally after sync)
rite index rebuild --if-needed
# → Only rebuilds if JSONL changed since last index
```

### Rebuild Logic

```rust
pub fn rebuild_index_from_jsonl(data_dir: &Path, index_path: &Path) -> Result<()> {
    // 1. Read all messages from all JSONL files
    let mut all_messages = Vec::new();
    for entry in glob(&format!("{}/**/*.jsonl", data_dir.display()))? {
        let messages = read_messages_from_jsonl(&entry)?;
        all_messages.extend(messages);
    }

    // 2. Dedupe by ULID
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for msg in all_messages {
        if seen.insert(msg.id.clone()) {
            deduped.push(msg);
        }
    }

    // 3. Sort by ULID (chronological)
    deduped.sort_by(|a, b| a.id.cmp(&b.id));

    // 4. Rebuild index
    let mut index = SearchIndex::open(index_path)?;
    index.clear()?;  // Drop and recreate tables
    index.index_messages(&deduped)?;

    Ok(())
}
```

## Attachment Handling

**Phase 1**: Reference-only (current plan)
- Attachments NOT synced via git
- Messages reference attachment paths
- Gracefully handle missing files:
  ```rust
  match fs::read(&attachment.path) {
      Ok(bytes) => display_attachment(bytes),
      Err(_) => display_placeholder("Attachment not available"),
  }
  ```

**Phase 2** (future): Separate sync
- Option A: Syncthing for attachments directory
- Option B: git-annex for content-addressed storage
- Option C: Restic backup (not sync, just recovery)

**Attachment metadata** (always synced):
- `.meta.json` files tracked in git
- Contains: original filename, MIME type, size, hash
- Telegram `file_id` for re-download if needed

## Merge Conflict Handling

**With union merge, conflicts are rare**. But if they happen:

### JSONL Conflicts (Shouldn't Happen)

Union merge prevents conflicts on JSONL files. If manual merge needed:
```bash
# Manual merge (rare)
rite sync --pull
# → git reports conflict on claims.jsonl
# → Read both versions
# → Combine and sort by timestamp
# → Dedupe by event ID
```

### State File Conflicts

`state.json` is gitignored (machine-local). If user manually tracks it:
- Use `merge=ours` strategy (keep local version)
- Or: Merge with max() for offsets/timestamps

### Binary File Conflicts

SQLite and attachments are gitignored. No conflicts.

## Error Handling

### Network Failures

```bash
rite sync --push
# → git push fails (network down)
# → Log error: "Sync failed: network unreachable"
# → Keep local commits, retry later
# → Exit code: 1

# User can retry manually
rite sync --push
```

### Merge Failures

```bash
rite sync --pull
# → git merge fails (unexpected conflict)
# → Abort merge: git merge --abort
# → Log error with git output
# → Suggest manual resolution
```

### Index Rebuild Failures

```bash
rite sync --pull
# → Merge succeeds
# → Index rebuild fails (disk full, corrupt JSONL)
# → Log error, leave old index
# → User can fix and run: rite index rebuild
```

## GitHub Free Tier

**What's free**:
- Unlimited private repos
- Unlimited commits/pushes
- 1GB storage (plenty for JSONL)
- 1GB/month bandwidth

**Cost estimate**:
- JSONL files: ~1KB per message
- 10,000 messages = ~10MB
- 100,000 messages = ~100MB
- Well within 1GB free tier

**No LFS needed**: JSONL is tiny, attachments not synced

## Implementation Phases

### Phase 1: Core Git Sync

**Commands**:
- `rite sync init [--remote <url>]` - Initialize git repo
- `rite sync` / `rite sync --push` - Push to remote
- `rite sync --pull` - Pull from remote
- `rite sync --pull --push` - Pull then push

**Auto-commit**:
- Hook into message send, claim, agent registration
- Git commit after each operation
- Commit message describes the operation

**Testing**:
- Two-machine sync simulation
- Union merge verification
- Conflict handling

### Phase 2: Index Rebuild

**Commands**:
- `rite index rebuild` - Full rebuild
- `rite index rebuild --if-needed` - Only if changed
- `rite index status` - Check if rebuild needed

**Auto-rebuild**:
- After `rite sync --pull`, check if JSONL changed
- Background rebuild (non-blocking)
- Progress indicator for large indexes

### Phase 3: Polish

**Features**:
- `rite sync status` - Show uncommitted changes, ahead/behind
- `rite sync log` - Show recent sync history
- `rite sync check` - Verify repo health
- Systemd timer / cron examples
- Documentation and examples

**Error handling**:
- Graceful network failures
- Clear error messages
- Recovery procedures

### Phase 4: Attachments (Future)

**Design options**:
- git-annex integration
- Syncthing configuration
- Reference-only with Telegram re-download

**Implementation**:
- `rite sync config --attachments [none|annex|syncthing]`
- Graceful handling of missing files
- Placeholder display in TUI

## Migration

**Existing Rite installs**:
```bash
# User runs init on existing data dir
rite sync init --remote git@github.com:user/rite-data.git
# → Detects existing JSONL files
# → Creates .gitattributes, .gitignore
# → Initial commit with all existing data
# → Push to remote
```

**Fresh install**:
```bash
# Clone existing data repo
rite sync clone git@github.com:user/rite-data.git
# → Clones to ~/.local/share/rite/
# → Rebuilds SQLite index from JSONL
# → Ready to use
```

## Security Considerations

**Private repos**: Use private GitHub repos for sensitive data

**Credentials**: Use SSH keys, not HTTPS with password

**Attachments**: Not synced (avoid leaking sensitive files)

**State files**: Machine-local (no sync of local state)

## Testing Strategy

### Unit Tests

- Union merge behavior (dedupe, sort)
- Index rebuild from JSONL
- Auto-commit logic
- Error handling (network, merge, disk)

### Integration Tests

- Two-machine sync simulation
- Concurrent writes → merge
- SQLite rebuild after merge
- Missing attachment handling

### Manual Testing

- Setup on fresh machine
- Daily workflow (send, claim, sync)
- Multi-machine scenarios
- Network failure recovery

## Metrics

**Track via logging**:
- Sync frequency (pushes per day)
- Sync size (commits, bytes transferred)
- Index rebuild time
- Merge conflicts (should be zero)
- Network errors and retries

## Summary

**What makes this simple**:
- ✅ Already event-sourced (append-only JSONL)
- ✅ Union merge prevents conflicts
- ✅ SQLite is just a projection (rebuildable)
- ✅ ULIDs ensure global uniqueness
- ✅ GitHub free tier is plenty

**User experience**:
```bash
# One-time setup
rite sync init --remote git@github.com:me/rite.git

# Daily usage (transparent)
rite send general "message"  # Auto-commits

# Periodic sync (manual or cron)
rite sync --pull --push      # Sync with remotes

# That's it!
```

**Cost**: $0 (GitHub free tier)

**Complexity**: Low (git + union merge)

**Reliability**: High (append-only, no conflicts)
