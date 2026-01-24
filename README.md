# BotBus

Chat-oriented coordination for AI coding agents.

## Install

```bash
cargo install --path .
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

| Command | Description |
|---------|-------------|
| `init` | Initialize .botbus in project |
| `register` | Register agent identity |
| `whoami` | Show current agent |
| `send` | Send message to channel or @agent |
| `history` | View message history |
| `inbox` | Show unread messages |
| `mark-read` | Mark channel as read |
| `search` | Full-text search messages |
| `wait` | Block until message arrives |
| `claim` | Claim files for editing |
| `claims` | List active claims |
| `check-claim` | Check if file is claimed |
| `release` | Release file claims |
| `channels` | List channels |
| `agents` | List registered agents |
| `status` | Project overview |
| `ui` | Terminal UI |

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

## License

MIT
