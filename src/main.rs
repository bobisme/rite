use anyhow::Result;
use clap::Parser;

use botbus::cli::{self, Cli, Commands, OutputFormat};
use botbus::core::project::ensure_data_dir;

fn main() -> Result<()> {
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

        Commands::Whoami => cli::whoami::run(cli.json, cli.agent.as_deref()),

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
            json: cli.json,
            agent: cli.agent.clone(),
        }),

        Commands::Watch { channel, all } => cli::watch::run(channel, all),

        Commands::Channels { mine } => cli::channels::run(cli.json, mine, cli.agent.as_deref()),

        Commands::Agents { active } => cli::agents::run(cli.json, active),

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

        Commands::Claims { all, mine } => {
            cli::claim::claims(format, all, mine, cli.agent.as_deref())
        }

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
            channel,
            count,
            mark_read,
        } => cli::inbox::run(
            cli::inbox::InboxOptions {
                channel,
                count,
                mark_read,
                format,
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
    }
}
