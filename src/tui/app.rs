use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::agent::Agent;
use crate::core::message::Message;
use crate::core::project::{agents_path, channel_path, channels_dir, state_path};
use crate::storage::jsonl::{read_last_n, read_records, read_records_from_offset};
use crate::storage::state::ProjectState;
use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};

use super::ui;

pub struct App {
    project_root: PathBuf,
    current_agent: Option<String>,
    channels: Vec<String>,
    selected_channel: usize,
    agents: Vec<Agent>,
    messages: Vec<Message>,
    input: String,
    message_scroll: usize,
    should_quit: bool,
    focus: Focus,
    channel_offset: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Channels,
    Messages,
    Input,
}

impl App {
    pub fn new(project_root: &Path, initial_channel: Option<String>) -> Result<Self> {
        let state = ProjectState::new(state_path(project_root));
        let current_agent = state.current_agent()?;

        let channels = list_channels(project_root)?;
        let agents: Vec<Agent> = read_records(&agents_path(project_root))?;

        // Find initial channel index
        let selected_channel = if let Some(ch) = &initial_channel {
            channels.iter().position(|c| c == ch).unwrap_or(0)
        } else {
            channels.iter().position(|c| c == "general").unwrap_or(0)
        };

        let mut app = Self {
            project_root: project_root.to_path_buf(),
            current_agent,
            channels,
            selected_channel,
            agents,
            messages: Vec::new(),
            input: String::new(),
            message_scroll: 0,
            should_quit: false,
            focus: Focus::Input,
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
        match self.focus {
            Focus::Input => match key {
                KeyCode::Esc => self.focus = Focus::Messages,
                KeyCode::Enter => self.send_message()?,
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                KeyCode::Tab => self.focus = Focus::Channels,
                _ => {}
            },
            Focus::Messages => match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('i') | KeyCode::Enter => self.focus = Focus::Input,
                KeyCode::Tab => self.focus = Focus::Channels,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.message_scroll = self.message_scroll.saturating_add(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.message_scroll = self.message_scroll.saturating_sub(1);
                }
                KeyCode::Esc => self.should_quit = true,
                _ => {}
            },
            Focus::Channels => match key {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Tab | KeyCode::Enter => self.focus = Focus::Input,
                KeyCode::Esc => self.focus = Focus::Messages,
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.selected_channel > 0 {
                        self.selected_channel -= 1;
                        self.load_messages()?;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.selected_channel < self.channels.len().saturating_sub(1) {
                        self.selected_channel += 1;
                        self.load_messages()?;
                    }
                }
                _ => {}
            },
        }
        Ok(())
    }

    fn send_message(&mut self) -> Result<()> {
        if self.input.trim().is_empty() {
            return Ok(());
        }

        let agent = match &self.current_agent {
            Some(a) => a.clone(),
            None => return Ok(()),
        };

        let channel = match self.current_channel() {
            Some(c) => c,
            None => return Ok(()),
        };

        let msg = Message::new(&agent, &channel, &self.input);
        let path = channel_path(&self.project_root, &channel);
        crate::storage::jsonl::append_record(&path, &msg)?;

        self.input.clear();
        self.refresh_messages()?;

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
        self.channels.get(self.selected_channel).cloned()
    }

    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    pub fn selected_channel_index(&self) -> usize {
        self.selected_channel
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn input(&self) -> &str {
        &self.input
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
}

fn list_channels(project_root: &Path) -> Result<Vec<String>> {
    let channels = channels_dir(project_root);
    let mut result = Vec::new();

    if channels.exists() {
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    // Skip DM channels for now
                    if !name.starts_with("_dm_") {
                        result.push(name.to_string());
                    }
                }
            }
        }
    }

    // Ensure general is first
    result.sort();
    if let Some(pos) = result.iter().position(|c| c == "general") {
        result.remove(pos);
        result.insert(0, "general".to_string());
    }

    Ok(result)
}
