use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use ratatui::prelude::*;
use tui_textarea::TextArea;

use std::path::Path;
use std::time::Duration;

use crate::core::identity::resolve_agent;
use crate::core::message::Message;
use crate::core::project::{channel_path, channels_dir, state_path};
use crate::storage::jsonl::{read_last_n, read_records_from_offset};
use crate::storage::state::ProjectState;
use crate::storage::watch::{debounce_events, filter_channel_events, watch_directory};

use super::ui;

pub struct App {
    current_agent: Option<String>,
    channels: Vec<String>,
    dm_channels: Vec<String>,
    selected_channel: usize,
    messages: Vec<Message>,
    message_scroll: usize,
    should_quit: bool,
    focus: Focus,
    channel_offset: u64,
    show_help: bool,
    /// Cached viewport height for page scroll calculations
    viewport_height: usize,
    /// Cached layout areas for mouse click detection
    channels_area: Rect,
    messages_area: Rect,
    /// File sizes when TUI started (for new message indicators)
    initial_sizes: std::collections::HashMap<String, u64>,
    /// Current new message counts per channel
    new_message_counts: std::collections::HashMap<String, usize>,
    /// Previous new message counts (for detecting changes to trigger notifications)
    previous_message_counts: std::collections::HashMap<String, usize>,
    /// Timestamp when current channel was focused (for auto-clear timer)
    channel_focused_at: Option<std::time::Instant>,
    /// Which channels have visible separators
    separator_visible: std::collections::HashSet<String>,
    /// Input textarea for composing messages
    pub input: TextArea<'static>,
    /// Cached input area for mouse click detection
    input_area: Rect,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Focus {
    Channels,
    Messages,
    Input,
}

impl App {
    pub fn new(initial_channel: Option<String>) -> Result<Self> {
        // Get agent from env var (no explicit flag for TUI)
        let current_agent = resolve_agent(None);

        let (channels, dm_channels) = list_channels()?;

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

        // Capture initial file sizes for new message indicators
        let initial_sizes = capture_channel_sizes(&channels, &dm_channels);

        let mut app = Self {
            current_agent,
            channels,
            dm_channels,
            selected_channel,
            messages: Vec::new(),
            message_scroll: 0,
            should_quit: false,
            focus: Focus::Channels,
            channel_offset: 0,
            show_help: false,
            viewport_height: 20,
            channels_area: Rect::default(),
            messages_area: Rect::default(),
            initial_sizes,
            new_message_counts: std::collections::HashMap::new(),
            previous_message_counts: std::collections::HashMap::new(),
            channel_focused_at: None,
            separator_visible: std::collections::HashSet::new(),
            input: TextArea::default(),
            input_area: Rect::default(),
        };

        app.update_new_message_counts();
        app.load_messages()?;

        Ok(app)
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        // Enable mouse capture
        crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;

        // Set up file watcher
        let channels_path = channels_dir();
        let (_watcher, rx) = watch_directory(&channels_path)?;

        let result = self.run_loop(terminal, &rx);

        // Disable mouse capture on exit
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);

        result
    }

    fn run_loop<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    ) -> Result<()> {
        loop {
            terminal.draw(|f| ui::draw(f, self))?;

            // Handle input first for responsiveness
            if event::poll(Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key.code, key.modifiers)?;
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse)?;
                    }
                    _ => {}
                }
            }

            if self.should_quit {
                break;
            }

            // Check for file changes (non-blocking, short timeout)
            let changes = debounce_events(rx, Duration::from_millis(1));
            let channel_changes = filter_channel_events(changes);

            // Check if any changed channels are new (not in our current list)
            let has_new_channels = channel_changes
                .iter()
                .any(|ch| !self.channels.contains(ch) && !self.dm_channels.contains(ch));

            if has_new_channels {
                // Refresh channel list to pick up new channels
                self.refresh_channels()?;
            }

            // Update unread counts for all channels when any channel changes
            if !channel_changes.is_empty() {
                self.update_new_message_counts();
            }

            if let Some(current) = self.current_channel() {
                if channel_changes.contains(&current) {
                    self.refresh_messages()?;
                }

                // Check if separator should auto-clear (after 2 seconds of viewing)
                if let Some(focused_at) = self.channel_focused_at
                    && focused_at.elapsed() >= Duration::from_secs(2)
                {
                    self.clear_separator_and_mark_read(&current);
                }
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // Dismiss help overlay on any key
        if self.show_help {
            self.show_help = false;
            return Ok(());
        }

        let ctrl = modifiers.contains(KeyModifiers::CONTROL);

        // Global keys (with ctrl modifier check)
        if let KeyCode::Char('q') = key
            && (ctrl || self.focus != Focus::Input)
        {
            // Ctrl+Q quits from anywhere, plain 'q' quits when not in input
            self.should_quit = true;
            return Ok(());
        }

        match key {
            KeyCode::Esc => {
                // Esc clears input if focused, otherwise quits
                if self.focus == Focus::Input {
                    self.input = TextArea::default();
                } else {
                    self.should_quit = true;
                }
                return Ok(());
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Channels => Focus::Messages,
                    Focus::Messages => Focus::Input,
                    Focus::Input => Focus::Channels,
                };
                return Ok(());
            }
            KeyCode::Char('h') if self.focus != Focus::Input => {
                self.focus = Focus::Channels;
                return Ok(());
            }
            KeyCode::Char('l') if self.focus != Focus::Input => {
                self.focus = Focus::Messages;
                return Ok(());
            }
            KeyCode::Char('?') if self.focus != Focus::Input => {
                self.show_help = true;
                return Ok(());
            }
            _ => {}
        }

        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let half_page = self.viewport_height / 2;
        let full_page = self.viewport_height;

        // Focus-specific keys
        match self.focus {
            Focus::Messages => match key {
                // Single line scroll
                KeyCode::Up | KeyCode::Char('k') if !ctrl => {
                    self.message_scroll = self.message_scroll.saturating_add(1);
                }
                KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                    self.message_scroll = self.message_scroll.saturating_sub(1);
                }
                // 10 line scroll with Ctrl
                KeyCode::Char('k') if ctrl => {
                    self.message_scroll = self.message_scroll.saturating_add(10);
                }
                KeyCode::Char('j') if ctrl => {
                    self.message_scroll = self.message_scroll.saturating_sub(10);
                }
                // Half page scroll (vim u/d)
                KeyCode::Char('u') => {
                    self.message_scroll = self.message_scroll.saturating_add(half_page);
                }
                KeyCode::Char('d') => {
                    self.message_scroll = self.message_scroll.saturating_sub(half_page);
                }
                // Full page scroll (less b/f)
                KeyCode::Char('b') | KeyCode::PageUp => {
                    self.message_scroll = self.message_scroll.saturating_add(full_page);
                }
                KeyCode::Char('f') | KeyCode::PageDown => {
                    self.message_scroll = self.message_scroll.saturating_sub(full_page);
                }
                // Jump to top/bottom
                KeyCode::Home | KeyCode::Char('g') => {
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
            Focus::Input => {
                let ctrl = modifiers.contains(KeyModifiers::CONTROL);

                match key {
                    // Ctrl+S sends the message
                    KeyCode::Char('s') if ctrl => {
                        self.send_input_message()?;
                    }
                    // Forward all other keys to tui-textarea (including Enter for newlines)
                    _ => {
                        use tui_textarea::Input;
                        self.input.input(Input {
                            key: key.into(),
                            ctrl: modifiers.contains(KeyModifiers::CONTROL),
                            alt: modifiers.contains(KeyModifiers::ALT),
                            shift: modifiers.contains(KeyModifiers::SHIFT),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let x = mouse.column;
                let y = mouse.row;

                // Check if click is in channels area
                if x >= self.channels_area.x
                    && x < self.channels_area.x + self.channels_area.width
                    && y >= self.channels_area.y
                    && y < self.channels_area.y + self.channels_area.height
                {
                    self.focus = Focus::Channels;

                    // Calculate which channel was clicked (accounting for border)
                    let clicked_row = (y - self.channels_area.y).saturating_sub(1) as usize;
                    let total = self.total_channel_count();

                    // Account for DM separator if present
                    let dm_separator_offset =
                        if !self.dm_channels.is_empty() && clicked_row >= self.channels.len() {
                            1 // Skip the "-- DMs --" separator
                        } else {
                            0
                        };

                    let adjusted_row = if clicked_row > self.channels.len() {
                        clicked_row.saturating_sub(dm_separator_offset)
                    } else {
                        clicked_row
                    };

                    if adjusted_row < total {
                        self.selected_channel = adjusted_row;
                        self.load_messages()?;
                    }
                }
                // Check if click is in messages area
                else if x >= self.messages_area.x
                    && x < self.messages_area.x + self.messages_area.width
                    && y >= self.messages_area.y
                    && y < self.messages_area.y + self.messages_area.height
                {
                    self.focus = Focus::Messages;
                }
                // Check if click is in input area
                else if x >= self.input_area.x
                    && x < self.input_area.x + self.input_area.width
                    && y >= self.input_area.y
                    && y < self.input_area.y + self.input_area.height
                {
                    self.focus = Focus::Input;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.focus == Focus::Messages {
                    self.message_scroll = self.message_scroll.saturating_add(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.focus == Focus::Messages {
                    self.message_scroll = self.message_scroll.saturating_sub(3);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn load_messages(&mut self) -> Result<()> {
        if let Some(channel) = self.current_channel() {
            let path = channel_path(&channel);
            self.messages = read_last_n(&path, 100)?;
            self.channel_offset = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            self.message_scroll = 0;

            // Start timer for separator auto-clear when viewing channel with new messages
            if self.new_message_count(&channel) > 0 {
                self.channel_focused_at = Some(std::time::Instant::now());
                self.separator_visible.insert(channel.clone());
            } else {
                self.channel_focused_at = None;
                self.separator_visible.remove(&channel);
            }
        }
        Ok(())
    }

    fn refresh_messages(&mut self) -> Result<()> {
        if let Some(channel) = self.current_channel() {
            let path = channel_path(&channel);
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

        // Update new message counts for all channels
        self.update_new_message_counts();

        // If current channel now has new messages, show separator
        if let Some(channel) = self.current_channel()
            && self.new_message_count(&channel) > 0
            && !self.separator_visible.contains(&channel)
        {
            self.channel_focused_at = Some(std::time::Instant::now());
            self.separator_visible.insert(channel);
        }

        Ok(())
    }

    fn refresh_channels(&mut self) -> Result<()> {
        let (channels, dm_channels) = list_channels()?;

        // Preserve the current selection if possible
        let current_name = self.current_channel();

        self.channels = channels;
        self.dm_channels = dm_channels;

        // Try to maintain selection on the same channel
        if let Some(name) = current_name {
            if let Some(idx) = self.channels.iter().position(|c| c == &name) {
                self.selected_channel = idx;
            } else if let Some(idx) = self.dm_channels.iter().position(|c| c == &name) {
                self.selected_channel = self.channels.len() + idx;
            } else {
                // Channel disappeared, reset to first
                self.selected_channel = 0;
            }
        }

        // Update initial sizes for new channels
        for channel in self.channels.iter().chain(self.dm_channels.iter()) {
            if !self.initial_sizes.contains_key(channel) {
                let path = channel_path(channel);
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                self.initial_sizes.insert(channel.clone(), size);
            }
        }

        Ok(())
    }

    fn clear_separator_and_mark_read(&mut self, channel: &str) {
        // Mark all messages as read by updating initial_sizes
        let path = channel_path(channel);
        let current_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.initial_sizes.insert(channel.to_string(), current_size);

        // Clear the separator and counters
        self.separator_visible.remove(channel);
        self.new_message_counts.remove(channel);
        self.previous_message_counts.remove(channel);
        self.channel_focused_at = None;
    }

    fn update_new_message_counts(&mut self) {
        let all_channels: Vec<String> = self
            .channels
            .iter()
            .chain(self.dm_channels.iter())
            .cloned()
            .collect();

        let current_channel = self.current_channel();

        for channel in all_channels {
            let path = channel_path(&channel);
            let current_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let initial_size = self.initial_sizes.get(&channel).copied().unwrap_or(0);

            let previous_count = self
                .previous_message_counts
                .get(&channel)
                .copied()
                .unwrap_or(0);

            if current_size > initial_size {
                // Count newlines (messages) in the new portion
                let count = count_new_messages(&path, initial_size, current_size);
                if count > 0 {
                    self.new_message_counts.insert(channel.clone(), count);

                    // Send notification if this is a background channel with new messages
                    if Some(&channel) != current_channel.as_ref() && count > previous_count {
                        if let Err(e) = self.send_notification(&channel, count - previous_count) {
                            eprintln!("Failed to send notification for #{}: {}", channel, e);
                        }
                    }
                }
            } else {
                self.new_message_counts.remove(&channel);
            }
        }

        // Update previous counts for next iteration
        self.previous_message_counts = self.new_message_counts.clone();
    }

    fn send_notification(&self, channel: &str, new_count: usize) -> Result<()> {
        use notify_rust::Notification;

        // Read the latest message to show in notification (best effort)
        let path = channel_path(channel);
        let messages: Vec<Message> = read_last_n(&path, 1).unwrap_or_default();

        let (summary, body) = if let Some(msg) = messages.last() {
            let summary = format!("#{}", channel);
            let body = if msg.body.len() > 100 {
                format!("{}: {}...", msg.agent, &msg.body[..97])
            } else {
                format!("{}: {}", msg.agent, msg.body)
            };
            (summary, body)
        } else {
            let summary = format!("#{}", channel);
            let body = format!(
                "{} new message{}",
                new_count,
                if new_count == 1 { "" } else { "s" }
            );
            (summary, body)
        };

        // Try to send notification, ignore errors (notifications are best-effort)
        match Notification::new()
            .summary(&summary)
            .body(&body)
            .timeout(10000)
            .show()
        {
            Ok(_) => Ok(()),
            Err(e) => {
                // Log but don't fail - notifications might not be available
                eprintln!(
                    "Notification failed (this is normal on some systems): {}",
                    e
                );
                Ok(())
            }
        }
    }

    pub fn new_message_count(&self, channel: &str) -> usize {
        self.new_message_counts.get(channel).copied().unwrap_or(0)
    }

    pub fn has_separator(&self, channel: &str) -> bool {
        self.separator_visible.contains(channel)
    }

    pub fn separator_position(&self, channel: &str) -> Option<usize> {
        // Calculate how many messages are "old" (before initial_sizes)
        let initial_size = self.initial_sizes.get(channel).copied()?;
        if initial_size == 0 {
            return None; // All messages are new, no separator needed
        }

        // Count messages that were present when TUI started
        let path = channel_path(channel);
        let current_size = std::fs::metadata(&path).map(|m| m.len()).ok()?;

        if current_size <= initial_size {
            return None; // No new messages
        }

        // Count new messages
        let new_count = count_new_messages(&path, initial_size, current_size);
        if new_count == 0 || new_count >= self.messages.len() {
            return None; // All messages are new or counting failed
        }

        // Messages Vec is oldest-first chronologically
        // Separator should appear BEFORE the first new message
        // If we have 5 messages and 2 are new, separator goes at index 3
        // (after old messages at 0,1,2 and before new messages at 3,4)
        let old_count = self.messages.len().saturating_sub(new_count);
        if old_count == 0 {
            return None; // All messages are new
        }

        Some(old_count)
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

    fn send_input_message(&mut self) -> Result<()> {
        use crate::core::identity::require_agent;
        use crate::core::message::Message;
        use crate::core::project::channel_path;
        use crate::storage::jsonl::append_record;

        // Get message text from input (join lines and trim)
        let text = self.input.lines().join("\n").trim().to_string();

        // Don't send empty messages
        if text.is_empty() {
            return Ok(());
        }

        // Get current channel
        let channel = match self.current_channel() {
            Some(ch) => ch,
            None => return Ok(()), // No channel selected
        };

        // Get user from $USER environment variable
        let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        let agent_name = require_agent(Some(&user))?;

        // Create and send message directly (without CLI output)
        let msg = Message::new(&agent_name, &channel, &text).with_labels(vec!["human".to_string()]);
        let path = channel_path(&channel);
        append_record(&path, &msg)?;

        // Evaluate channel hooks in a background thread so OnExit hooks
        // don't freeze the TUI event loop.
        {
            let ch = channel.clone();
            let mid = msg.id.to_string();
            let meta = msg.meta.clone();
            let agent = agent_name.clone();
            std::thread::spawn(move || {
                crate::cli::hooks::evaluate_hooks(&ch, &mid, meta.as_ref(), &agent);
            });
        }

        // Clear input after sending
        self.input = TextArea::default();

        // Refresh messages to show the new message
        self.refresh_messages()?;

        Ok(())
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

    /// Clamp scroll offset to valid range based on rendered line count.
    /// Called by UI after layout and line wrapping is calculated.
    pub fn clamp_scroll_lines(&mut self, max_scroll: usize, viewport_height: usize) {
        self.viewport_height = viewport_height;
        self.message_scroll = self.message_scroll.min(max_scroll);
    }

    pub fn show_help(&self) -> bool {
        self.show_help
    }

    /// Update cached layout areas for mouse click detection
    pub fn set_layout_areas(&mut self, channels: Rect, messages: Rect) {
        self.channels_area = channels;
        self.messages_area = messages;
    }

    /// Update cached input area for mouse click detection
    pub fn set_input_area(&mut self, input: Rect) {
        self.input_area = input;
    }
}

fn list_channels() -> Result<(Vec<String>, Vec<String>)> {
    let channels = channels_dir();
    let mut public_channels: Vec<(String, std::time::SystemTime)> = Vec::new();
    let mut dm_channels: Vec<(String, std::time::SystemTime)> = Vec::new();

    // Load closed channels list from state
    let state_file = ProjectState::new(state_path());
    let state = state_file.load()?;
    let closed_channels = &state.closed_channels;

    if channels.exists() {
        for entry in std::fs::read_dir(&channels)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(name) = path.file_stem().and_then(|s| s.to_str())
            {
                // Skip closed channels
                if closed_channels.contains(&name.to_string()) {
                    continue;
                }

                let modified = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                if name.starts_with("_dm_") {
                    // Show ALL DMs - omniscient observer view
                    dm_channels.push((name.to_string(), modified));
                } else {
                    public_channels.push((name.to_string(), modified));
                }
            }
        }
    }

    // Sort alphabetically
    let mut public: Vec<String> = public_channels.into_iter().map(|(n, _)| n).collect();
    let mut dms: Vec<String> = dm_channels.into_iter().map(|(n, _)| n).collect();

    public.sort();
    dms.sort();

    // Keep #general at the top of public channels
    if let Some(pos) = public.iter().position(|c| c == "general") {
        let general = public.remove(pos);
        public.insert(0, general);
    }

    Ok((public, dms))
}

/// Capture initial file sizes for all channels
fn capture_channel_sizes(
    channels: &[String],
    dm_channels: &[String],
) -> std::collections::HashMap<String, u64> {
    let mut sizes = std::collections::HashMap::new();

    for channel in channels.iter().chain(dm_channels.iter()) {
        let path = channel_path(channel);
        if let Ok(meta) = std::fs::metadata(&path) {
            sizes.insert(channel.clone(), meta.len());
        }
    }

    sizes
}

/// Count new messages (lines) between two file offsets
fn count_new_messages(path: &Path, start: u64, end: u64) -> usize {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };

    if file.seek(SeekFrom::Start(start)).is_err() {
        return 0;
    }

    let bytes_to_read = (end - start) as usize;
    let mut buffer = vec![0u8; bytes_to_read.min(1024 * 1024)]; // Cap at 1MB

    let Ok(bytes_read) = file.read(&mut buffer) else {
        return 0;
    };

    // Count newlines
    buffer[..bytes_read].iter().filter(|&&b| b == b'\n').count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    const DATA_DIR_ENV_VAR: &str = "BOTBUS_DATA_DIR";

    #[test]
    #[serial]
    fn test_refresh_channels_picks_up_new_channels() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
            env::set_var("BOTBUS_AGENT", "test-agent");
        }

        // Create channels directory
        let channels_dir = temp.path().join("channels");
        fs::create_dir_all(&channels_dir).unwrap();

        // Create initial channel
        fs::write(
            channels_dir.join("general.jsonl"),
            r#"{"ts":"2024-01-01T00:00:00Z","id":"01HX0000000000000000000000","agent":"test","channel":"general","body":"hello"}"#,
        )
        .unwrap();

        // Create app - should see only general
        let mut app = App::new(None).unwrap();
        assert_eq!(app.channels.len(), 1);
        assert_eq!(app.channels[0], "general");

        // Create a new channel while app is "running"
        fs::write(
            channels_dir.join("new-channel.jsonl"),
            r#"{"ts":"2024-01-01T00:01:00Z","id":"01HX0000000000000000000001","agent":"test","channel":"new-channel","body":"hi"}"#,
        )
        .unwrap();

        // Refresh channels - should now see both
        app.refresh_channels().unwrap();
        assert_eq!(app.channels.len(), 2);
        assert!(app.channels.contains(&"general".to_string()));
        assert!(app.channels.contains(&"new-channel".to_string()));

        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
            env::remove_var("BOTBUS_AGENT");
        }
    }

    #[test]
    #[serial]
    fn test_refresh_channels_preserves_selection() {
        let temp = TempDir::new().unwrap();
        let temp_path = temp.path().to_str().unwrap();

        unsafe {
            env::set_var(DATA_DIR_ENV_VAR, temp_path);
            env::set_var("BOTBUS_AGENT", "test-agent");
        }

        let channels_dir = temp.path().join("channels");
        fs::create_dir_all(&channels_dir).unwrap();

        // Create two channels
        fs::write(channels_dir.join("general.jsonl"), "").unwrap();
        fs::write(channels_dir.join("backend.jsonl"), "").unwrap();

        let mut app = App::new(Some("backend".to_string())).unwrap();
        assert_eq!(app.selected_channel, 1); // backend is second after general

        // Create a new channel
        fs::write(channels_dir.join("frontend.jsonl"), "").unwrap();

        // Refresh - should still have backend selected
        let before_name = app.current_channel().unwrap();
        app.refresh_channels().unwrap();
        let after_name = app.current_channel().unwrap();

        assert_eq!(before_name, after_name);
        assert_eq!(after_name, "backend");

        unsafe {
            env::remove_var(DATA_DIR_ENV_VAR);
            env::remove_var("BOTBUS_AGENT");
        }
    }
}
