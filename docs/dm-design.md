# TUI Design: Omniscient Observer View

## Design Philosophy

The TUI is a **read-only monitoring dashboard** for observing agent coordination - not a chat client for participating in conversations.

Key principles:
- **Omniscient**: See ALL conversations, including DMs between other agents
- **Read-only**: No compose, send, or reply - agents use the CLI
- **Observer-first**: Designed for humans monitoring agent activity

## Current State

- **Storage**: DMs use `_dm_Agent1_Agent2` channel naming (alphabetically sorted)
- **TUI**: Shows channels and DMs in sidebar, messages in main pane

### Current Limitations

1. **DM filtering**: Only shows DMs involving "current agent" - wrong for observer view
2. **No activity overview**: Can't see recent activity across all conversations
3. **No new message indicators**: Hard to spot where activity is happening

## Design Options

### Option A: Unified Conversation List (Recommended)

Treat all conversations equally - channels and DMs are just "conversations":

```
┌─ Conversations ─┐
│ #general    (3) │  <- 3 new messages
│ #backend        │
│ Alice↔Bob   (1) │  <- DM between Alice and Bob
│ CLI↔Storage     │  <- DM between CLIAgent and StorageFixer
│ #frontend       │
└─────────────────┘
```

- Sort by recent activity (most recent at top) or alphabetically
- Show ALL DMs regardless of current agent identity
- Format DMs as `Agent1↔Agent2` to distinguish from channels
- Unread counts show new messages since TUI opened

### Option B: Activity Feed

Single chronological view of ALL messages across ALL conversations:

```
┌─ Activity Feed ──────────────────────────────┐
│ [21:23] #general     Alice: Starting work... │
│ [21:24] Alice↔Bob    Alice: Quick question   │
│ [21:24] Alice↔Bob    Bob: Sure, what's up?   │
│ [21:25] #general     Carol: Done with PR     │
│ [21:25] #backend     Alice: API ready        │
└──────────────────────────────────────────────┘
```

- All messages interleaved by timestamp
- Channel/DM name shown inline
- Click/select to filter to that conversation
- Good for "what just happened?" view

### Option C: Hybrid (Conversation List + Activity Feed)

Toggle between views with a keybinding:
- `v` to toggle between "Conversations" and "Activity Feed"
- Or split: sidebar shows conversations, main pane can show either single conversation or activity feed

## Recommendation: Option A with Activity Feed as Future Enhancement

### Phase 1: Fix DM Visibility
1. Remove current-agent filtering - show ALL DM channels
2. Format as `Agent1↔Agent2` instead of `@OtherAgent`
3. Sort conversations by most recent activity

### Phase 2: New Message Indicators
1. Track file sizes when TUI opens
2. Show count of new messages per conversation
3. Bold/highlight conversations with new activity

### Phase 3: Activity Feed (Future)
1. Add toggle (`a` key?) for unified activity view
2. Interleave all messages chronologically
3. Color-code by conversation

## Implementation Notes

### Remove Agent Filtering

Current code in `app.rs`:
```rust
// REMOVE this filter:
if let Some(agent) = current_agent {
    if dm_involves_agent(name, agent) {
        dm_channels.push(name.to_string());
    }
}

// REPLACE with:
dm_channels.push(name.to_string());
```

### DM Display Format

```rust
// In ui.rs, format DM channels as "Agent1↔Agent2"
fn format_dm_name(channel: &str) -> String {
    if let Some((a, b)) = dm_agents(channel) {
        format!("{}↔{}", a, b)
    } else {
        channel.to_string()
    }
}
```

### Sort by Activity

```rust
// Sort channels by last modified time
channels.sort_by_key(|ch| {
    let path = channel_path(project_root, ch);
    std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
});
channels.reverse(); // Most recent first
```

## Data Model

No changes needed to storage - just presentation layer changes.

## Open Questions

1. **Conversation grouping**: Keep channels and DMs separate, or fully unified?
   - Recommend: Unified, sorted by activity

2. **Agent pane**: Keep showing registered agents, or repurpose?
   - Recommend: Keep as-is, useful context for who's participating

3. **Current agent identity**: Still needed for anything?
   - Only for the Agents pane (highlight "you") - optional
