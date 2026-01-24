# DM Viewing Design for TUI

## Current State

DMs are already partially supported:
- **Storage**: DMs use `_dm_Agent1_Agent2` channel naming (alphabetically sorted)
- **CLI**: `botbus send @AgentName "message"` creates/appends to DM channels
- **TUI**: DMs appear in channel list under "-- DMs --" separator, shown as `@OtherAgent`

### Current Limitations

1. **Discovery**: DMs only show if they involve the current agent (by design)
2. **No unread indicators**: Can't see which DMs have new messages
3. **Mixed with channels**: DMs are in the same list as public channels
4. **No quick compose**: Must use CLI to start a new DM

## Design Options

### Option A: Enhanced Channel List (Recommended)

Keep the current integrated approach but add:

1. **Unread counts** next to each channel/DM:
   ```
   Channels ─────────
   #general        (3)
   #backend
   -- DMs --
   @Alice          (1)
   @Bob
   ```

2. **Visual distinction**: DMs in magenta (already done), unread in bold

3. **Quick DM compose**: Press `@` in TUI to start typing an agent name, autocomplete from registered agents, creates/opens DM

**Pros**: Minimal UI change, familiar pattern (Slack/Discord sidebar)
**Cons**: Can get crowded with many DMs

### Option B: Tabbed Interface

Add tabs at the top: `[Channels] [DMs] [Agents]`

- Tab key or number keys (1/2/3) to switch
- Each tab has its own list
- DMs tab shows all DM conversations

**Pros**: Clean separation, scales better
**Cons**: More navigation, loses at-a-glance view

### Option C: Split Pane

Horizontal split: Channels on top, DMs on bottom (within sidebar)

```
┌─ Channels ─┐
│ #general   │
│ #backend   │
├─── DMs ────┤
│ @Alice     │
│ @Bob       │
└────────────┘
```

**Pros**: Always visible, clear separation
**Cons**: Reduces space for each, may need scrolling

## Recommendation: Option A (Enhanced Channel List)

The current implementation is close - we just need to add:

### Phase 1: Unread Indicators
1. Track read position per channel (already have `AgentState.read_offsets`)
2. Compare current file size to read offset
3. Show unread count in channel list

### Phase 2: Quick DM Compose
1. Press `@` to enter "DM mode"
2. Type agent name with fuzzy autocomplete
3. Enter opens/creates DM channel

### Phase 3: Notifications (Future)
1. Desktop notifications for mentions
2. Badge count in terminal title

## Implementation Notes

### Unread Count Calculation

```rust
// In app.rs, add to App struct:
unread_counts: HashMap<String, usize>,

// Calculate on load/refresh:
fn calculate_unread(&mut self, channel: &str) -> usize {
    let path = channel_path(&self.project_root, channel);
    let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let read_offset = self.agent_state.get_read_offset(channel).unwrap_or(0);
    
    if file_size > read_offset {
        // Count newlines between read_offset and file_size
        // (each message is one line in JSONL)
        count_lines_in_range(&path, read_offset, file_size)
    } else {
        0
    }
}
```

### Quick DM Mode

```rust
// Add to Focus enum:
enum Focus {
    Channels,
    Messages,
    DmCompose,  // New
}

// In DmCompose mode:
// - Show input overlay
// - Filter agent list as user types
// - Enter selects top match and opens DM
// - Escape cancels
```

## Data Model

No changes needed - current DM channel naming (`_dm_Agent1_Agent2`) works well:
- Deterministic (sorted names)
- Self-documenting
- Easy to parse
- Works with existing JSONL storage

## Open Questions

1. **Mark as read**: Automatic when viewing, or explicit command?
   - Recommend: Auto-mark when scrolled to bottom

2. **DM with unregistered agent**: Allow or require registration?
   - Recommend: Allow (agent might register later)

3. **Group DMs**: Support `_dm_Agent1_Agent2_Agent3`?
   - Recommend: Defer, current 1:1 DMs sufficient for MVP
