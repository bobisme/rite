use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};

use crate::cli::send;
use crate::core::message::Message;
use crate::core::project::{channel_path, channels_dir, state_path};
use crate::storage::jsonl::read_records_from_offset;
use crate::storage::state::ProjectState;
use crate::telegram::client::{TelegramClient, TelegramMessage, Update};
use crate::telegram::config::{TelegramConfig, TelegramConfigStore};

const POLL_TIMEOUT_SECS: u64 = 30;
const SYNC_INTERVAL: Duration = Duration::from_secs(60);
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
const TELEGRAM_MAX_CHARS: usize = 4000;
const SYSTEM_CHANNEL_PREFIX: char = '_';
/// Maximum incoming message size (10KB) to prevent memory exhaustion
const MAX_INCOMING_MESSAGE_LEN: usize = 10 * 1024;

pub async fn run(config: TelegramConfig, store: TelegramConfigStore) -> Result<()> {
    let client = TelegramClient::new(&config.bot_token)?;
    let config = Arc::new(Mutex::new(config));

    // Shutdown signal - sender held here, receivers given to tasks
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Install Ctrl+C handler
    let shutdown_signal = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_signal.send(true);
        }
    });

    // Initial topic sync
    {
        let mut guard = config.lock().await;
        if sync_topics(&client, &mut guard).await? {
            store.save(&guard)?;
        }
    }

    // Spawn poll task
    let poll_handle = {
        let client = client.clone();
        let config = Arc::clone(&config);
        let store = store.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move { poll_loop(client, config, store, shutdown_rx).await })
    };

    // Spawn watch task
    let watch_handle = {
        let client = client.clone();
        let config = Arc::clone(&config);
        let store = store.clone();
        let shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move { watch_loop(client, config, store, shutdown_rx).await })
    };

    // Wait for either task to complete (or fail)
    tokio::select! {
        result = poll_handle => {
            let _ = shutdown_tx.send(true);
            result??;
        }
        result = watch_handle => {
            let _ = shutdown_tx.send(true);
            result??;
        }
    }

    Ok(())
}

async fn poll_loop(
    client: TelegramClient,
    config: Arc<Mutex<TelegramConfig>>,
    store: TelegramConfigStore,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let mut backoff = Duration::from_secs(1);
    let mut offset = {
        let guard = config.lock().await;
        guard.last_update_id.map(|id| id + 1)
    };

    loop {
        // Check shutdown before starting a new poll
        if *shutdown_rx.borrow() {
            break;
        }

        // Race between get_updates and shutdown signal
        let updates_result = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                break;
            }
            result = client.get_updates(offset, POLL_TIMEOUT_SECS) => result,
        };

        match updates_result {
            Ok(updates) => {
                backoff = Duration::from_secs(1);
                if updates.is_empty() {
                    continue;
                }

                for update in updates {
                    offset = Some(update.update_id + 1);
                    if let Err(err) = handle_update(&client, &config, &store, update).await {
                        eprintln!("Telegram update error: {err}");
                    }
                }

                if let Some(next_offset) = offset {
                    let mut guard = config.lock().await;
                    guard.last_update_id = Some(next_offset - 1);
                    store.save(&guard)?;
                }
            }
            Err(err) => {
                eprintln!("Telegram getUpdates failed: {err}");

                // Backoff sleep with shutdown check
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }

                backoff = std::cmp::min(backoff.saturating_mul(2), Duration::from_secs(30));
            }
        }
    }

    Ok(())
}

async fn handle_update(
    client: &TelegramClient,
    config: &Arc<Mutex<TelegramConfig>>,
    store: &TelegramConfigStore,
    update: Update,
) -> Result<()> {
    let message = extract_message(update);
    let Some(message) = message else {
        return Ok(());
    };

    let Some(text) = message.text.clone() else {
        return Ok(());
    };

    let Some(from) = &message.from else {
        return Ok(());
    };

    let (chat_id, owner_user_id, agent_name, topic_map) = {
        let guard = config.lock().await;
        (
            guard.chat_id,
            guard.owner_user_id,
            guard.agent_name.clone(),
            guard.channel_topics.clone(),
        )
    };

    if message.chat.id != chat_id {
        return Ok(());
    }

    if from.id != owner_user_id {
        return Ok(());
    }

    let Some(thread_id) = message.message_thread_id else {
        client
            .send_message(chat_id, None, "Please post in a topic.")
            .await?;
        return Ok(());
    };

    if let Some(command) = parse_command(&text) {
        return handle_command(client, config, store, chat_id, thread_id, command).await;
    }

    let channel = channel_for_topic(&topic_map, thread_id);
    let Some(channel) = channel else {
        client
            .send_message(
                chat_id,
                Some(thread_id),
                "No channel mapped to this topic yet.",
            )
            .await?;
        return Ok(());
    };

    // Validate incoming message
    if let Some(error_msg) = validate_incoming_message(&text) {
        client
            .send_message(chat_id, Some(thread_id), &error_msg)
            .await?;
        return Ok(());
    }

    // Send to bus (this is sync, but quick)
    send::run(
        channel,
        text,
        None,
        vec!["human".to_string(), "telegram".to_string()],
        vec![],
        Some(&agent_name),
    )?;
    Ok(())
}

async fn handle_command(
    client: &TelegramClient,
    config: &Arc<Mutex<TelegramConfig>>,
    store: &TelegramConfigStore,
    chat_id: i64,
    thread_id: i64,
    command: String,
) -> Result<()> {
    let reply = {
        let mut guard = config.lock().await;
        let channel = channel_for_topic(&guard.channel_topics, thread_id)
            .unwrap_or_else(|| "unknown".to_string());

        let (reply, should_save) = match command.as_str() {
            "mute" => {
                if guard.muted_topics.insert(thread_id) {
                    (
                        format!("Muted Telegram notifications for #{channel}."),
                        true,
                    )
                } else {
                    (format!("Already muted for #{channel}."), false)
                }
            }
            "unmute" => {
                if guard.muted_topics.remove(&thread_id) {
                    (
                        format!("Unmuted Telegram notifications for #{channel}."),
                        true,
                    )
                } else {
                    (format!("Not muted for #{channel}."), false)
                }
            }
            _ => ("Unknown command. Use /mute or /unmute.".to_string(), false),
        };

        if should_save {
            store.save(&guard)?;
        }

        reply
    };

    client
        .send_message(chat_id, Some(thread_id), &reply)
        .await?;
    Ok(())
}

fn extract_message(update: Update) -> Option<TelegramMessage> {
    update.message.or(update.edited_message)
}

fn parse_command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    if !first.starts_with('/') {
        return None;
    }

    let raw = first.trim_start_matches('/');
    let command = raw.split('@').next().unwrap_or(raw);
    if command.is_empty() {
        None
    } else {
        Some(command.to_string())
    }
}

async fn watch_loop(
    client: TelegramClient,
    config: Arc<Mutex<TelegramConfig>>,
    store: TelegramConfigStore,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let channels_dir = channels_dir();
    if !channels_dir.exists() {
        // Wait for shutdown if no channels dir
        let _ = shutdown_rx.changed().await;
        return Ok(());
    }

    let mut offsets = HashMap::new();
    let mut active = active_channels_set()?;

    for channel in &active {
        offsets.insert(channel.clone(), channel_len(channel));
    }

    let mut last_sync = tokio::time::Instant::now();
    let mut interval = tokio::time::interval(WATCH_INTERVAL);

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => break,
            _ = interval.tick() => {}
        }

        // Periodic topic sync
        if last_sync.elapsed() >= SYNC_INTERVAL {
            let mut guard = config.lock().await;
            if sync_topics(&client, &mut guard).await? {
                store.save(&guard)?;
            }
            drop(guard);

            active = active_channels_set()?;
            for channel in &active {
                offsets
                    .entry(channel.clone())
                    .or_insert_with(|| channel_len(channel));
            }
            last_sync = tokio::time::Instant::now();
        }

        // Check for new channels and messages
        let current_channels = active_channels_set()?;

        for channel in &current_channels {
            if !is_eligible_channel(channel) {
                continue;
            }

            // New channel appeared
            if !active.contains(channel) {
                if is_channel_closed(channel)? {
                    continue;
                }

                active.insert(channel.clone());
                offsets
                    .entry(channel.clone())
                    .or_insert_with(|| channel_len(channel));
                ensure_topic_for_channel(&client, &config, &store, channel).await?;
            }

            // Check for new messages
            let path = channel_path(channel);
            let offset = offsets.get(channel).copied().unwrap_or(0);
            match read_records_from_offset::<Message>(&path, offset) {
                Ok((messages, new_offset)) => {
                    for msg in &messages {
                        if let Err(err) = publish_message(&client, &config, &store, msg).await {
                            eprintln!("Failed to publish to Telegram: {err}");
                        }
                    }
                    offsets.insert(channel.clone(), new_offset);
                }
                Err(err) => {
                    eprintln!("Failed to read channel updates: {err}");
                }
            }
        }
    }

    Ok(())
}

async fn publish_message(
    client: &TelegramClient,
    config: &Arc<Mutex<TelegramConfig>>,
    store: &TelegramConfigStore,
    msg: &Message,
) -> Result<()> {
    if !is_eligible_channel(&msg.channel) {
        return Ok(());
    }

    let (chat_id, agent_name, mut topic_id, mut muted) = {
        let guard = config.lock().await;
        (
            guard.chat_id,
            guard.agent_name.clone(),
            guard.channel_topics.get(&msg.channel).copied(),
            guard
                .channel_topics
                .get(&msg.channel)
                .map(|id| guard.muted_topics.contains(id))
                .unwrap_or(false),
        )
    };

    if msg.agent == agent_name {
        return Ok(());
    }

    if topic_id.is_none() {
        ensure_topic_for_channel(client, config, store, &msg.channel).await?;
        let guard = config.lock().await;
        topic_id = guard.channel_topics.get(&msg.channel).copied();
        muted = guard
            .channel_topics
            .get(&msg.channel)
            .map(|id| guard.muted_topics.contains(id))
            .unwrap_or(false);
    }

    let Some(topic_id) = topic_id else {
        return Ok(());
    };

    if muted {
        return Ok(());
    }

    let text = format_outbound_message(msg);
    client.send_message(chat_id, Some(topic_id), &text).await
}

fn format_outbound_message(msg: &Message) -> String {
    let mut text = format!("{}: {}", msg.agent, msg.body);

    if !msg.labels.is_empty() {
        text.push_str("\nlabels: ");
        text.push_str(&msg.labels.join(", "));
    }

    if !msg.attachments.is_empty() {
        let names: Vec<String> = msg.attachments.iter().map(|a| a.name.clone()).collect();
        text.push_str("\nattachments: ");
        text.push_str(&names.join(", "));
    }

    truncate_text(&text, TELEGRAM_MAX_CHARS)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let truncated: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{}...", truncated)
}

fn channel_for_topic(map: &HashMap<String, i64>, thread_id: i64) -> Option<String> {
    map.iter().find_map(|(channel, id)| {
        if *id == thread_id {
            Some(channel.clone())
        } else {
            None
        }
    })
}

fn active_channels_set() -> Result<HashSet<String>> {
    let channels = list_active_channels()?;
    Ok(channels.into_iter().collect())
}

fn list_active_channels() -> Result<Vec<String>> {
    let dir = channels_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let closed = ProjectState::new(state_path()).load()?.closed_channels;
    let closed_set: HashSet<String> = closed.into_iter().collect();

    let mut channels = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl")
            && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            && !closed_set.contains(name)
            && is_eligible_channel(name)
        {
            channels.push(name.to_string());
        }
    }

    channels.sort();
    Ok(channels)
}

fn is_channel_closed(channel: &str) -> Result<bool> {
    let state = ProjectState::new(state_path()).load()?;
    Ok(state.closed_channels.iter().any(|c| c == channel))
}

fn channel_len(channel: &str) -> u64 {
    std::fs::metadata(channel_path(channel))
        .map(|m| m.len())
        .unwrap_or(0)
}

async fn ensure_topic_for_channel(
    client: &TelegramClient,
    config: &Arc<Mutex<TelegramConfig>>,
    store: &TelegramConfigStore,
    channel: &str,
) -> Result<()> {
    if !is_eligible_channel(channel) {
        return Ok(());
    }

    let mut guard = config.lock().await;
    if guard.channel_topics.contains_key(channel) {
        return Ok(());
    }

    match client.create_forum_topic(guard.chat_id, channel).await {
        Ok(topic_id) => {
            guard.channel_topics.insert(channel.to_string(), topic_id);
            guard.topic_titles.insert(topic_id, channel.to_string());
            store.save(&guard)?;
        }
        Err(err) => {
            eprintln!("Failed to create Telegram topic for #{channel}: {err}");
        }
    }

    Ok(())
}

async fn sync_topics(client: &TelegramClient, config: &mut TelegramConfig) -> Result<bool> {
    let channels = list_active_channels()?;
    let active: HashSet<String> = channels.iter().cloned().collect();
    let mut changed = false;

    for channel in &channels {
        match config.channel_topics.get(channel).copied() {
            Some(topic_id) => match config.topic_titles.get(&topic_id) {
                Some(title) if title == channel => {}
                None => {
                    config.topic_titles.insert(topic_id, channel.to_string());
                    changed = true;
                }
                _ => {
                    if let Err(err) = client
                        .edit_forum_topic(config.chat_id, topic_id, channel)
                        .await
                        && !is_topic_not_modified_error(&err)
                    {
                        eprintln!("Failed to rename Telegram topic for #{channel}: {err}");
                    } else {
                        config.topic_titles.insert(topic_id, channel.to_string());
                        changed = true;
                    }
                }
            },
            None => match client.create_forum_topic(config.chat_id, channel).await {
                Ok(topic_id) => {
                    config.channel_topics.insert(channel.clone(), topic_id);
                    config.topic_titles.insert(topic_id, channel.to_string());
                    changed = true;
                }
                Err(err) => {
                    eprintln!("Failed to create Telegram topic for #{channel}: {err}");
                }
            },
        }
    }

    let removed_topics: Vec<i64> = config
        .channel_topics
        .iter()
        .filter(|(channel, _)| !active.contains(*channel))
        .map(|(_, topic_id)| *topic_id)
        .collect();

    if !removed_topics.is_empty() {
        config
            .channel_topics
            .retain(|channel, _| active.contains(channel));
        config
            .muted_topics
            .retain(|topic_id| !removed_topics.contains(topic_id));
        config
            .topic_titles
            .retain(|topic_id, _| !removed_topics.contains(topic_id));
        changed = true;
    }

    Ok(changed)
}

fn is_eligible_channel(channel: &str) -> bool {
    !channel.starts_with(SYSTEM_CHANNEL_PREFIX)
}

/// Validate incoming Telegram message. Returns error message if invalid, None if valid.
fn validate_incoming_message(text: &str) -> Option<String> {
    // Check length limit to prevent memory exhaustion
    if text.len() > MAX_INCOMING_MESSAGE_LEN {
        return Some(format!(
            "Message too long ({} bytes, max {} bytes)",
            text.len(),
            MAX_INCOMING_MESSAGE_LEN
        ));
    }

    // Check for null bytes which could cause issues in downstream processing
    if text.contains('\0') {
        return Some("Message contains invalid characters (null bytes)".to_string());
    }

    None
}

fn is_topic_not_modified_error(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("not modified") || message.contains("not_modified")
}
