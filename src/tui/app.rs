use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::agent::Agent;
use crate::core::identity::resolve_agent;
use crate::core::message::Message;
use crate::core::project::{agents_path, channel_path, channels_dir};
use crate::storage::jsonl::{read_last_n, read_records, read_records_from_offset};
use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};

use super::ui;

pub struct App {
    project_root: PathBuf,
    current_agent: Option<String>,
    channels: Vec<String>,
    dm_channels: Vec<String>,
    selected_channel: usize,
    agents: Vec<Agent>,
    messages: Vec<Message>,
    message_scroll: usize,
    should_quit: bool,
    focus: Focus,
    channel_offset: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Channels,
    Messages,
}

impl App {
    pub fn new(project_root: &Path, initial_channel: Option<String>) -> Result<Self> {
        // Get agent from env var (no explicit flag for TUI)
        let current_agent = resolve_agent(None, project_root);

        let (channels, dm_channels) = list_channels(project_root, current_agent.as_deref())?;
        let agents: Vec<Agent> = read_records(&agents_path(project_root))?;

        // Find initial channel index (search in both channels and DMs)
        let selected_channel = if let Some(ch) = &initial_channel {
            channels
                .iter()
                .position(|c| c == ch)
                .or_else(|| {
                    dm_channels
                        .iter()
                        .position(|c| c == ch)
                        .map(|i| i + channels.len())
                })
                .unwrap_or(0)
        } else {
            channels.iter().position(|c| c == "general").unwrap_or(0)
        };

        let mut app = Self {
            project_root: project_root.to_path_buf(),
            current_agent,
            channels,
            dm_channels,
            selected_channel,
            agents,
            messages: Vec::new(),
            message_scroll: 0,
            should_quit: false,
            focus: Focus::Messages,
            channel_offset: 0,
        };

        app.load_messages()?;

        Ok(app)
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B::Error: Send + Sync + 'static,
    {
        // Set up file watcher
        let channels_path = channels_dir(&self.project_root);
        let (_watcher, rx) = watch_directory(&channels_path)?;

        loop {
            terminal.draw(|f| ui::draw(f, self))?;

            // Check for file changes (non-blocking)
            let changes = debounce_events(&rx, Duration::from_millis(50));
            let channel_changes = filter_channel_events(changes);

            if let Some(current) = self.current_channel() {
                if channel_changes.contains(&current) {
                    self.refresh_messages()?;
                }
            }

            // Handle input
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key.code)?;
                    }
                }
            }

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyCode) -> Result<()> {
        // Global keys
        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
                return Ok(());
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Channels => Focus::Messages,
                    Focus::Messages => Focus::Channels,
                };
                return Ok(());
            }
            _ => {}
        }

        // Focus-specific keys
        match self.focus {
            Focus::Messages => match key {
                KeyCode::Up | KeyCode::Char('k') => {
                    // Scroll up = see older messages = increase scroll offset
                    // Actual clamping happens in UI after viewport height is known
                    self.message_scroll = self.message_scroll.saturating_add(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    // Scroll down = see newer messages = decrease scroll offset
                    self.message_scroll = self.message_scroll.saturating_sub(1);
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    // Scroll to oldest - use large value, UI will clamp
                    self.message_scroll = usize::MAX / 2;
                }
                KeyCode::End | KeyCode::Char('G') => {
                    self.message_scroll = 0;
                }
                _ => {}
            },
            Focus::Channels => match key {
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.selected_channel > 0 {
                        self.selected_channel -= 1;
                        self.load_messages()?;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let total = self.total_channel_count();
                    if self.selected_channel < total.saturating_sub(1) {
                        self.selected_channel += 1;
                        self.load_messages()?;
                    }
                }
                KeyCode::Enter => {
                    self.focus = Focus::Messages;
                }
                _ => {}
            },
        }
        Ok(())
    }

    fn load_messages(&mut self) -> Result<()> {
        if let Some(channel) = self.current_channel() {
            let path = channel_path(&self.project_root, &channel);
            self.messages = read_last_n(&path, 100)?;
            self.channel_offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            self.message_scroll = 0;
        }
        Ok(())
    }

    fn refresh_messages(&mut self) -> Result<()> {
        if let Some(channel) = self.current_channel() {
            let path = channel_path(&self.project_root, &channel);
            let (new_msgs, new_offset): (Vec<Message>, u64) =
                read_records_from_offset(&path, self.channel_offset)?;

            self.messages.extend(new_msgs);
            self.channel_offset = new_offset;

            // Keep only last 100
            if self.messages.len() > 100 {
                let drain_count = self.messages.len() - 100;
                self.messages.drain(0..drain_count);
            }
        }
        Ok(())
    }

    pub fn current_channel(&self) -> Option<String> {
        let total_public = self.channels.len();
        if self.selected_channel < total_public {
            self.channels.get(self.selected_channel).cloned()
        } else {
            self.dm_channels
                .get(self.selected_channel - total_public)
                .cloned()
        }
    }

    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    pub fn dm_channels(&self) -> &[String] {
        &self.dm_channels
    }

    pub fn selected_channel_index(&self) -> usize {
        self.selected_channel
    }

    /// Total number of channels (public + DMs)
    pub fn total_channel_count(&self) -> usize {
        self.channels.len() + self.dm_channels.len()
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn current_agent(&self) -> Option<&str> {
        self.current_agent.as_deref()
    }

    pub fn message_scroll(&self) -> usize {
        self.message_scroll
    }

    /// Clamp scroll offset to valid range based on viewport height.
    /// Called by UI after layout is known.
    pub fn clamp_scroll(&mut self, viewport_height: usize) {
        let max_scroll = self.messages.len().saturating_sub(viewport_height);
        self.message_scroll = self.message_scroll.min(max_scroll);
    }
}

fn list_channels(
    project_root: &Path,
    current_agent: Option<&str>,
) -> Result<(Vec<String>, Vec<String>)> {
    let channels = channels_dir(project_root);
    let mut public_channels = Vec::new();
    let mut dm_channels = Vec::new();

    if channels.exists() {
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if name.starts_with("_dm_") {
                        // Only include DMs that involve the current agent
                        if let Some(agent) = current_agent {
                            if dm_involves_agent(name, agent) {
                                dm_channels.push(name.to_string());
                            }
                        }
                    } else {
                        public_channels.push(name.to_string());
                    }
                }
            }
        }
    }

    // Ensure general is first in public channels
    public_channels.sort();
    if let Some(pos) = public_channels.iter().position(|c| c == "general") {
        public_channels.remove(pos);
        public_channels.insert(0, "general".to_string());
    }

    // Sort DM channels alphabetically
    dm_channels.sort();

    Ok((public_channels, dm_channels))
}

/// Check if a DM channel name involves the given agent
fn dm_involves_agent(channel_name: &str, agent: &str) -> bool {
    // DM channel format: _dm_Agent1_Agent2 (alphabetically sorted)
    let parts: Vec<&str> = channel_name
        .strip_prefix("_dm_")
        .unwrap_or("")
        .splitn(2, '_')
        .collect();
    parts.len() == 2 && (parts[0] == agent || parts[1] == agent)
}

/// Extract the other agent's name from a DM channel name
pub fn dm_other_agent(channel_name: &str, current_agent: &str) -> Option<String> {
    let parts: Vec<&str> = channel_name
        .strip_prefix("_dm_")
        .unwrap_or("")
        .splitn(2, '_')
        .collect();
    if parts.len() == 2 {
        if parts[0] == current_agent {
            Some(parts[1].to_string())
        } else if parts[1] == current_agent {
            Some(parts[0].to_string())
        } else {
            None
        }
    } else {
        None
    }
}
