use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use colored::Colorize;
use serde::Serialize;
use std::path::Path;

use crate::index::fts::SearchResult;
use crate::index::IndexSyncer;

pub struct SearchOptions {
    pub query: String,
    pub channel: Option<String>,
    pub count: usize,
    pub from: Option<String>,
    pub json: bool,
}

#[derive(Debug, Serialize)]
pub struct SearchOutput {
    pub query: String,
    pub count: usize,
    pub results: Vec<SearchResult>,
}

/// Full-text search messages.
pub fn run(options: SearchOptions, project_root: &Path) -> Result<()> {
    // Sync index first to include recent messages
    let mut syncer =
        IndexSyncer::new(project_root).with_context(|| "Failed to open search index")?;

    let stats = syncer.sync_all().with_context(|| "Failed to sync index")?;

    if stats.messages_indexed > 0 && !options.json {
        eprintln!(
            "{} Indexed {} new message(s)",
            "Info:".blue(),
            stats.messages_indexed
        );
    }

    // Build FTS query
    let fts_query = format!("body:{}", options.query);

    // Execute search
    let results = if let Some(channel) = &options.channel {
        syncer
            .index()
            .search_channel(&fts_query, channel, options.count)?
    } else if let Some(agent) = &options.from {
        syncer
            .index()
            .search_from(&fts_query, agent, options.count)?
    } else {
        syncer.index().search(&fts_query, options.count)?
    };

    if options.json {
        let output = SearchOutput {
            query: options.query,
            count: results.len(),
            results,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if results.is_empty() {
        println!("No messages found matching '{}'", options.query);
        return Ok(());
    }

    println!(
        "{} {} result(s) for '{}'",
        "Found:".green(),
        results.len(),
        options.query
    );
    println!();

    for result in &results {
        print_result(result);
    }

    Ok(())
}

fn print_result(result: &SearchResult) {
    // Parse timestamp
    let ts: DateTime<Utc> = result.ts.parse().unwrap_or_else(|_| Utc::now());
    let local_time: DateTime<Local> = ts.with_timezone(&Local);
    let time_str = local_time.format("%Y-%m-%d %H:%M").to_string();

    let agent_colored = colorize_agent(&result.agent);

    println!(
        "[{}] #{} {}: {}",
        time_str.dimmed(),
        result.channel.cyan(),
        agent_colored,
        result.body
    );
}

fn colorize_agent(name: &str) -> colored::ColoredString {
    let hash: usize = name.bytes().map(|b| b as usize).sum();
    let colors = [
        colored::Color::Cyan,
        colored::Color::Green,
        colored::Color::Yellow,
        colored::Color::Blue,
        colored::Color::Magenta,
    ];
    let color = colors[hash % colors.len()];
    name.color(color).bold()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{init, send};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let temp = TempDir::new().unwrap();
        init::run(false, temp.path()).unwrap();
        temp
    }

    #[test]
    fn test_search_basic() {
        let temp = setup();

        send::run_simple(
            "general".to_string(),
            "Hello world".to_string(),
            Some("Searcher"),
            temp.path(),
        )
        .unwrap();
        send::run_simple(
            "general".to_string(),
            "Working on authentication".to_string(),
            Some("Searcher"),
            temp.path(),
        )
        .unwrap();

        let options = SearchOptions {
            query: "auth*".to_string(),
            channel: None,
            count: 20,
            from: None,
            json: false,
        };

        run(options, temp.path()).unwrap();
    }

    #[test]
    fn test_search_json() {
        let temp = setup();

        send::run_simple(
            "general".to_string(),
            "Hello world".to_string(),
            Some("Searcher"),
            temp.path(),
        )
        .unwrap();

        let options = SearchOptions {
            query: "Hello".to_string(),
            channel: None,
            count: 20,
            from: None,
            json: true,
        };

        run(options, temp.path()).unwrap();
    }

    #[test]
    fn test_search_in_channel() {
        let temp = setup();

        send::run_simple(
            "general".to_string(),
            "Hello general".to_string(),
            Some("Searcher"),
            temp.path(),
        )
        .unwrap();
        send::run_simple(
            "backend".to_string(),
            "Hello backend".to_string(),
            Some("Searcher"),
            temp.path(),
        )
        .unwrap();

        let options = SearchOptions {
            query: "Hello".to_string(),
            channel: Some("backend".to_string()),
            count: 20,
            from: None,
            json: false,
        };

        run(options, temp.path()).unwrap();
    }

    #[test]
    fn test_search_no_results() {
        let temp = setup();

        let options = SearchOptions {
            query: "nonexistent".to_string(),
            channel: None,
            count: 20,
            from: None,
            json: false,
        };

        run(options, temp.path()).unwrap();
    }
}
