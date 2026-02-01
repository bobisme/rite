use anyhow::Result;
use clap::Parser;
use std::path::Path;

use botbus::cli::{self, Cli, Commands, OutputFormat};
use botbus::core::project::ensure_data_dir;

fn main() -> Result<()> {
    // Detect which binary name was used to invoke this program
    let _program_name = std::env::args()
        .next()
        .and_then(|arg0| {
            Path::new(&arg0)
                .file_name()
                .and_then(|name| name.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "bus".to_string());

    let cli = Cli::parse();

    // Resolve effective output format (--json flag overrides --format for backwards compatibility)
    let format = if cli.json {
        OutputFormat::Json
    } else {
        cli.format
    };

    // Ensure data directory exists for most commands
    // (init creates it explicitly, generate-name and doctor don't require it)
    if !matches!(
        cli.command,
        Commands::GenerateName | Commands::Init | Commands::Doctor
    ) {
        ensure_data_dir()?;
    }

    match cli.command {
        Commands::Init => cli::init::run(),

        Commands::Doctor => cli::doctor::run(format),

        Commands::GenerateName => {
            cli::names::run();
            Ok(())
        }

        Commands::Whoami => cli::whoami::run(format, cli.agent.as_deref()),

        Commands::Send {
            target,
            message,
            meta,
            labels,
            attachments,
        } => cli::send::run(
            target,
            message,
            meta,
            labels,
            attachments,
            cli.agent.as_deref(),
        ),

        Commands::History {
            channel,
            count,
            follow,
            timeout,
            follow_count,
            since,
            before,
            from,
            labels,
            after_offset,
            after_id,
            show_offset,
        } => cli::history::run(cli::history::HistoryOptions {
            channel,
            count,
            follow,
            timeout,
            follow_count,
            since,
            before,
            from,
            labels,
            after_offset,
            after_id,
            show_offset,
            format,
            agent: cli.agent.clone(),
        }),

        Commands::Watch { channel, all } => cli::watch::run(channel, all),

        Commands::Channels { command } => {
            use cli::ChannelsCommands;
            // Default to List if no subcommand provided (backward compatibility)
            match command.as_ref().unwrap_or(&ChannelsCommands::List {
                mine: false,
                all: false,
            }) {
                ChannelsCommands::List { mine, all } => {
                    cli::channels::list(format, *mine, *all, cli.agent.as_deref())
                }
                ChannelsCommands::Close { channel } => cli::channels::close(channel),
                ChannelsCommands::Reopen { channel } => cli::channels::reopen(channel),
                ChannelsCommands::Delete { channel } => cli::channels::delete(channel),
                ChannelsCommands::Rename { old_name, new_name } => {
                    cli::channels::rename(old_name, new_name)
                }
            }
        }

        Commands::Agents { active } => cli::agents::run(format, active),

        Commands::Search {
            query,
            channel,
            count,
            from,
        } => cli::search::run(cli::search::SearchOptions {
            query,
            channel,
            count,
            from,
            json: cli.json,
        }),

        Commands::Claim {
            patterns,
            ttl,
            message,
            extend,
        } => cli::claim::claim(cli::claim::ClaimOptions {
            patterns,
            ttl,
            message,
            extend,
            agent: cli.agent,
        }),

        Commands::Claims {
            all,
            mine,
            limit,
            since,
        } => cli::claim::claims(format, all, mine, limit, since, cli.agent.as_deref()),

        Commands::Release { patterns, all } => {
            cli::claim::release(patterns, all, cli.agent.as_deref())
        }

        Commands::CheckClaim { path } => {
            let safe = cli::claim::check_claim(path, format, cli.agent.as_deref())?;
            if !safe {
                std::process::exit(1);
            }
            Ok(())
        }

        Commands::Ui { channel } => cli::ui::run(channel),

        Commands::MarkRead {
            channel,
            offset,
            last_id,
        } => cli::mark_read::run(
            cli::mark_read::MarkReadOptions {
                channel,
                offset,
                last_id,
            },
            cli.agent.as_deref(),
        ),

        Commands::Inbox {
            channels,
            all,
            count,
            limit_per_channel,
            mark_read,
            mentions,
            count_only,
        } => cli::inbox::run(
            cli::inbox::InboxOptions {
                channels,
                count,
                limit_per_channel,
                mark_read,
                format,
                all,
                mentions,
                count_only,
            },
            cli.agent.as_deref(),
        ),

        Commands::Status => cli::status::run(format, cli.agent.as_deref()),

        Commands::Wait {
            mention,
            channel,
            labels,
            timeout,
        } => cli::wait::run(
            cli::wait::WaitOptions {
                mention,
                channel,
                labels,
                timeout,
                json: cli.json,
            },
            cli.agent.as_deref(),
        ),

        Commands::AgentsMd { command } => match command {
            cli::AgentsMdCommands::Init { file, remove } => cli::agentsmd::run_init(file, remove),
            cli::AgentsMdCommands::Show => cli::agentsmd::run_show(),
        },

        Commands::Subscribe { channel } => cli::subscribe::subscribe(channel, cli.agent.as_deref()),

        Commands::Unsubscribe { channel } => {
            cli::subscribe::unsubscribe(channel, cli.agent.as_deref())
        }

        Commands::Subscriptions => cli::subscribe::list_subscriptions(cli.agent.as_deref()),
    }
}
