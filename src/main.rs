use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use botbus::cli::{self, Cli, Commands};
use botbus::core::project::find_project_root;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { force } => {
            // For init, use current directory or specified project
            let project_root = cli
                .project
                .unwrap_or_else(|| std::env::current_dir().unwrap());
            cli::init::run(force, &project_root)
        }

        Commands::Register { name, description } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::register::run(name, description, &project_root)
        }

        Commands::Whoami => {
            let project_root = resolve_project_root(cli.project)?;
            cli::whoami::run(cli.json, cli.agent.as_deref(), &project_root)
        }

        Commands::Send {
            target,
            message,
            meta,
        } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::send::run(target, message, meta, cli.agent.as_deref(), &project_root)
        }

        Commands::History {
            channel,
            count,
            follow,
            since,
            before,
            from,
            after_offset,
            after_id,
            show_offset,
        } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::history::run(
                cli::history::HistoryOptions {
                    channel,
                    count,
                    follow,
                    since,
                    before,
                    from,
                    after_offset,
                    after_id,
                    show_offset,
                    json: cli.json,
                },
                &project_root,
            )
        }

        Commands::Watch { channel, all } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::watch::run(channel, all, &project_root)
        }

        Commands::Channels { all } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::channels::run(cli.json, all, &project_root)
        }

        Commands::Agents { active } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::agents::run(cli.json, active, &project_root)
        }

        Commands::Search {
            query,
            channel,
            count,
            from,
        } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::search::run(
                cli::search::SearchOptions {
                    query,
                    channel,
                    count,
                    from,
                },
                &project_root,
            )
        }

        Commands::Claim {
            patterns,
            ttl,
            message,
        } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::claim::claim(
                cli::claim::ClaimOptions {
                    patterns,
                    ttl,
                    message,
                    agent: cli.agent,
                },
                &project_root,
            )
        }

        Commands::Claims { all, mine } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::claim::claims(all, mine, cli.agent.as_deref(), &project_root)
        }

        Commands::Release { patterns, all } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::claim::release(patterns, all, cli.agent.as_deref(), &project_root)
        }

        Commands::CheckClaim { path } => {
            let project_root = resolve_project_root(cli.project)?;
            let safe =
                cli::claim::check_claim(path, cli.json, cli.agent.as_deref(), &project_root)?;
            if !safe {
                std::process::exit(1);
            }
            Ok(())
        }

        Commands::Ui { channel } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::ui::run(channel, &project_root)
        }

        Commands::MarkRead {
            channel,
            offset,
            last_id,
        } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::mark_read::run(
                cli::mark_read::MarkReadOptions {
                    channel,
                    offset,
                    last_id,
                },
                cli.agent.as_deref(),
                &project_root,
            )
        }

        Commands::Inbox {
            channel,
            count,
            mark_read,
        } => {
            let project_root = resolve_project_root(cli.project)?;
            cli::inbox::run(
                cli::inbox::InboxOptions {
                    channel,
                    count,
                    mark_read,
                },
                cli.agent.as_deref(),
                &project_root,
            )
        }

        Commands::Status => {
            let project_root = resolve_project_root(cli.project)?;
            cli::status::run(cli.json, cli.agent.as_deref(), &project_root)
        }
    }
}

/// Resolve the project root, either from the CLI option or by searching upward.
fn resolve_project_root(project: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = project {
        return Ok(path);
    }

    let cwd = std::env::current_dir().context("Failed to get current directory")?;

    find_project_root(&cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "Not in a BotBus project.\n\n\
             Run 'botbus init' to initialize BotBus in this directory,\n\
             or use '--project <PATH>' to specify a different location."
        )
    })
}
