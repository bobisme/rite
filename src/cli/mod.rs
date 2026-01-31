use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub mod agents;
pub mod agentsmd;
pub mod channels;
pub mod claim;
pub mod doctor;
pub mod format;
pub mod history;
pub mod inbox;
pub mod init;
pub mod mark_read;
pub mod names;
pub mod search;
pub mod send;
pub mod status;
pub mod subscribe;
pub mod ui;
pub mod wait;
pub mod watch;
pub mod whoami;

/// Output format for structured data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text
    Text,
    /// JSON - standard machine-readable format
    Json,
    /// TOON - Text-Only Object Notation, optimized for AI agents (default)
    #[default]
    Toon,
}

#[derive(Parser)]
#[command(name = "botbus")]
#[command(author, version, about = "Chat-oriented coordination for AI coding agents", long_about = None)]
pub struct Cli {
    /// Agent identity (default: from BOTBUS_AGENT env var)
    #[arg(short, long, global = true, env = "BOTBUS_AGENT")]
    pub agent: Option<String>,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Increase verbosity
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Output in JSON format (where applicable) [deprecated: use --format json]
    #[arg(long, global = true)]
    pub json: bool,

    /// Output format: toon (default), json, or text
    #[arg(long, global = true, value_enum, default_value = "toon")]
    pub format: OutputFormat,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize the BotBus data directory
    Init,

    /// Check environment health and configuration
    Doctor,

    /// Generate a random agent name (kebab-case)
    GenerateName,

    /// Display current agent identity
    Whoami,

    /// Send a message to a channel or agent
    #[command(alias = "post")]
    Send {
        /// Channel name or @agent for DM
        target: String,

        /// Message content
        message: String,

        /// Attach metadata (JSON)
        #[arg(long)]
        meta: Option<String>,

        /// Add label(s) to the message (can be used multiple times)
        #[arg(short = 'L', long = "label", action = clap::ArgAction::Append)]
        labels: Vec<String>,

        /// Attach file(s) (can be used multiple times)
        #[arg(long = "attach", action = clap::ArgAction::Append)]
        attachments: Vec<String>,
    },

    /// View message history
    #[command(aliases = &["read", "show"])]
    History {
        /// Channel to view (default: general)
        channel: Option<String>,

        /// Number of messages (default: 50)
        #[arg(short = 'n', long, alias = "limit", default_value = "50")]
        count: usize,

        /// Follow mode (like tail -f)
        #[arg(short, long)]
        follow: bool,

        /// Exit follow mode after N seconds
        #[arg(long)]
        timeout: Option<u64>,

        /// Exit follow mode after receiving N new messages
        #[arg(long)]
        follow_count: Option<usize>,

        /// Messages after this time
        #[arg(long)]
        since: Option<String>,

        /// Messages before this time
        #[arg(long)]
        before: Option<String>,

        /// Filter by sender
        #[arg(long)]
        from: Option<String>,

        /// Filter by label (can be used multiple times - messages must have ANY of the labels)
        #[arg(short = 'L', long = "label", action = clap::ArgAction::Append)]
        labels: Vec<String>,

        /// Read messages after this byte offset (for incremental reading)
        #[arg(long)]
        after_offset: Option<u64>,

        /// Read messages after this message ID (ULID)
        #[arg(long)]
        after_id: Option<String>,

        /// Show offset info for next read
        #[arg(long)]
        show_offset: bool,
    },

    /// Stream new messages in real-time
    Watch {
        /// Channel to watch (default: all)
        channel: Option<String>,

        /// Watch all channels
        #[arg(long)]
        all: bool,
    },

    /// Manage channels
    #[command(aliases = &["list-channels", "ls"])]
    Channels {
        #[command(subcommand)]
        command: Option<ChannelsCommands>,
    },

    /// List agents (derived from message history)
    #[command(alias = "list-agents")]
    Agents {
        /// Only show recently active agents
        #[arg(long)]
        active: bool,
    },

    /// Full-text search messages
    Search {
        /// Search query (supports FTS5 syntax)
        query: String,

        /// Limit to channel
        #[arg(short, long)]
        channel: Option<String>,

        /// Max results (default: 20)
        #[arg(short = 'n', long, default_value = "20")]
        count: usize,

        /// Filter by sender
        #[arg(long)]
        from: Option<String>,
    },

    /// Claim files for editing (advisory lock)
    Claim {
        /// Glob patterns to claim (relative paths expanded to absolute)
        #[arg(required_unless_present = "extend")]
        patterns: Vec<String>,

        /// Time-to-live in seconds (default: 3600)
        #[arg(short, long, default_value = "3600")]
        ttl: u64,

        /// Optional message about the claim
        #[arg(short, long)]
        message: Option<String>,

        /// Extend TTL on existing claims matching these patterns
        #[arg(long, conflicts_with = "patterns")]
        extend: Option<String>,
    },

    /// List active file claims
    #[command(alias = "list-claims")]
    Claims {
        /// Include expired claims
        #[arg(long)]
        all: bool,

        /// Only show my claims
        #[arg(long)]
        mine: bool,

        /// Limit output to N most recent claims
        #[arg(short = 'n', long)]
        limit: Option<usize>,

        /// Show claims created after this time (e.g., "2h ago", "2026-01-28")
        #[arg(long)]
        since: Option<String>,
    },

    /// Release file claims
    Release {
        /// Patterns to release (default: all your claims)
        patterns: Vec<String>,

        /// Release all your claims
        #[arg(long)]
        all: bool,
    },

    /// Check if a file is claimed by another agent
    CheckClaim {
        /// File path or pattern to check
        path: String,
    },

    /// Launch the terminal UI
    Ui {
        /// Start in this channel
        #[arg(short, long)]
        channel: Option<String>,
    },

    /// Mark a channel as read (for incremental reading)
    MarkRead {
        /// Channel to mark as read
        channel: String,

        /// Explicit byte offset (default: current end of file)
        #[arg(long)]
        offset: Option<u64>,

        /// Explicit last message ID
        #[arg(long)]
        last_id: Option<String>,
    },

    /// Show unread messages (uses stored read cursor)
    Inbox {
        /// Specific channel(s) to check (default: DMs only)
        #[arg(short, long, action = clap::ArgAction::Append)]
        channels: Vec<String>,

        /// Check all channels
        #[arg(long)]
        all: bool,

        /// Maximum total messages to show across all channels
        #[arg(short = 'n', long, default_value = "50")]
        count: usize,

        /// Maximum messages to show per channel
        #[arg(long)]
        limit_per_channel: Option<usize>,

        /// Mark as read after displaying
        #[arg(long)]
        mark_read: bool,

        /// Check all channels for @mentions of current agent
        #[arg(long)]
        mentions: bool,

        /// Only show the count of unread messages (no message content)
        #[arg(long)]
        count_only: bool,
    },

    /// Show status overview
    Status,

    /// Wait for a message (blocking, with optional timeout)
    Wait {
        /// Wait for @mention of current agent
        #[arg(long)]
        mention: bool,

        /// Wait for messages in specific channel
        #[arg(short, long)]
        channel: Option<String>,

        /// Wait for messages with specific label(s) (can be used multiple times)
        #[arg(short = 'L', long = "label", action = clap::ArgAction::Append)]
        labels: Vec<String>,

        /// Timeout in seconds (0 = no timeout)
        #[arg(short, long, default_value = "0")]
        timeout: u64,
    },

    /// Manage AGENTS.md workflow instructions
    #[command(name = "agentsmd")]
    AgentsMd {
        #[command(subcommand)]
        command: AgentsMdCommands,
    },

    /// Subscribe to a channel
    Subscribe {
        /// Channel to subscribe to
        channel: String,
    },

    /// Unsubscribe from a channel
    Unsubscribe {
        /// Channel to unsubscribe from
        channel: String,
    },

    /// List subscribed channels
    Subscriptions,
}

#[derive(Subcommand)]
pub enum ChannelsCommands {
    /// List all channels
    List {
        /// Only show channels you've participated in (sent or mentioned)
        #[arg(long)]
        mine: bool,

        /// Show all channels including closed ones
        #[arg(long)]
        all: bool,
    },

    /// Close a channel (hide from listings, preserves history)
    Close {
        /// Channel to close
        channel: String,
    },

    /// Reopen a closed channel
    Reopen {
        /// Channel to reopen
        channel: String,
    },

    /// Delete a channel permanently (admin only)
    Delete {
        /// Channel to delete
        channel: String,
    },

    /// Rename a channel (admin only)
    Rename {
        /// Current channel name
        old_name: String,
        /// New channel name
        new_name: String,
    },
}

#[derive(Subcommand)]
pub enum AgentsMdCommands {
    /// Generate or update AGENTS.md with BotBus workflow instructions
    Init {
        /// Explicit file path (default: auto-detect AGENTS.md, CLAUDE.md, etc.)
        #[arg(long)]
        file: Option<PathBuf>,

        /// Remove BotBus instructions instead of adding/updating
        #[arg(long)]
        remove: bool,
    },

    /// Print the BotBus section that would be added to AGENTS.md
    Show,
}
