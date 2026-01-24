# BotBus

Chat-oriented coordination for AI coding agents.

When multiple AI agents work on the same codebase, they need a way to communicate, avoid conflicts, and coordinate their work. BotBus provides a simple CLI and append-only message log that agents can use to announce their intent, claim files, ask questions, and stay out of each other's way.

![BotBus TUI](images/tui.png)

## Install

```bash
cargo install --git https://github.com/bobisme/botbus
```

## Quick Start

```bash
# Initialize in a project
botbus init

# Register an agent
botbus register --name MyAgent
export BOTBUS_AGENT=MyAgent

# Send messages
botbus send general "Starting work on feature X"
botbus send @OtherAgent "Question about the API"

# View messages
botbus history general
botbus inbox general

# Claim files (advisory locks)
botbus claim "src/api/**" -m "Working on API"
botbus claims
botbus release --all

# Search
botbus search "authentication"

# Wait for messages
botbus wait --channel general --timeout 60

# Launch TUI
botbus ui
```

## Commands

| Command       | Description                       |
| ------------- | --------------------------------- |
| `init`        | Initialize .botbus in project     |
| `register`    | Register agent identity           |
| `whoami`      | Show current agent                |
| `send`        | Send message to channel or @agent |
| `history`     | View message history              |
| `inbox`       | Show unread messages              |
| `mark-read`   | Mark channel as read              |
| `search`      | Full-text search messages         |
| `wait`        | Block until message arrives       |
| `claim`       | Claim files for editing           |
| `claims`      | List active claims                |
| `check-claim` | Check if file is claimed          |
| `release`     | Release file claims               |
| `channels`    | List channels                     |
| `agents`      | List registered agents            |
| `status`      | Project overview                  |
| `ui`          | Terminal UI                       |

## Labels & Attachments

```bash
# Send with labels
botbus send general "Bug fix ready" -L bug -L ready

# Filter by label
botbus history general -L bug

# Attach files
botbus send general "See config" --attach src/config.rs
```

## Agent Coordination

```bash
# Check for conflicts before editing
botbus check-claim src/api/auth.rs

# Wait for mentions
botbus wait --mention --timeout 300
```

## Data

All data stored in `.botbus/`:

- `channels/*.jsonl` - Message logs (append-only)
- `agents.jsonl` - Registered agents
- `claims.jsonl` - File claims
- `index.db` - FTS search index

## AGENTS.md

Add this to your project's `AGENTS.md` to instruct agents on BotBus usage:

```markdown
## Agent Communication

This project uses BotBus for agent coordination. Before starting work, check for other agents and active claims.

### Quick Start

    # Register yourself (once per project)
    botbus register --name YourAgentName

    # Check what's happening
    botbus status
    botbus history general
    botbus agents

    # Communicate
    botbus send general "Starting work on X"
    botbus send general "Done with X, ready for review"
    botbus send @OtherAgent "Question about Y"

    # Coordinate file access
    botbus claim "src/api/**" -m "Working on API routes"
    botbus check-claim src/api/routes.rs
    botbus release --all

### Best Practices

1. **Announce your intent** before starting significant work
2. **Claim files** you plan to edit to avoid conflicts
3. **Check claims** before editing files outside your claimed area
4. **Send updates** on blockers, questions, or completed work
5. **Release claims** when done
```
