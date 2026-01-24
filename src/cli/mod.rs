use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod agents;
pub mod channels;
pub mod claim;
pub mod history;
pub mod inbox;
pub mod init;
pub mod mark_read;
pub mod register;
pub mod search;
pub mod send;
pub mod ui;
pub mod watch;
pub mod whoami;

#[derive(Parser)]
#[command(name = "botbus")]
#[command(author, version, about = "Chat-oriented coordination for AI coding agents", long_about = None)]
pub struct Cli {
    /// Project directory (default: auto-detect)
    #[arg(short, long, global = true, env = "BOTBUS_PROJECT")]
    pub project: Option<PathBuf>,

    /// Agent identity (default: from BOTBUS_AGENT env var)
    #[arg(short, long, global = true, env = "BOTBUS_AGENT")]
    pub agent: Option<String>,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Increase verbosity
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Output in JSON format (where applicable)
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize BotBus in a project directory
    Init {
        /// Overwrite existing .botbus directory
        #[arg(long)]
        force: bool,
    },

    /// Register an agent identity in the current project
    Register {
        /// Agent name (auto-generated if omitted)
        #[arg(short, long)]
        name: Option<String>,

        /// Optional description
        #[arg(short, long)]
        description: Option<String>,
    },

    /// Display current agent identity
    Whoami,

    /// Send a message to a channel or agent
    Send {
        /// Channel name or @agent for DM
        target: String,

        /// Message content
        message: String,

        /// Attach metadata (JSON)
        #[arg(long)]
        meta: Option<String>,
    },

    /// View message history
    History {
        /// Channel to view (default: general)
        channel: Option<String>,

        /// Number of messages (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        count: usize,

        /// Follow mode (like tail -f)
        #[arg(short, long)]
        follow: bool,

        /// Messages after this time
        #[arg(long)]
        since: Option<String>,

        /// Messages before this time
        #[arg(long)]
        before: Option<String>,

        /// Filter by sender
        #[arg(long)]
        from: Option<String>,

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

    /// List all channels
    Channels {
        /// Include DM channels
        #[arg(long)]
        all: bool,
    },

    /// List registered agents
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
        /// Glob patterns to claim
        #[arg(required = true)]
        patterns: Vec<String>,

        /// Time-to-live in seconds (default: 3600)
        #[arg(short, long, default_value = "3600")]
        ttl: u64,

        /// Optional message about the claim
        #[arg(short, long)]
        message: Option<String>,
    },

    /// List active file claims
    Claims {
        /// Include expired claims
        #[arg(long)]
        all: bool,

        /// Only show my claims
        #[arg(long)]
        mine: bool,
    },

    /// Release file claims
    Release {
        /// Patterns to release (default: all your claims)
        patterns: Vec<String>,

        /// Release all your claims
        #[arg(long)]
        all: bool,
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
        /// Channel to check (default: general)
        channel: Option<String>,

        /// Maximum messages to show
        #[arg(short = 'n', long, default_value = "50")]
        count: usize,

        /// Mark as read after displaying
        #[arg(long)]
        mark_read: bool,
    },
}
