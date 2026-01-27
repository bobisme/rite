use chrono::{DateTime, Local};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::app::{App, Focus};

/// Colors matching lazygit style
const ACTIVE_BORDER: Color = Color::Green;
const INACTIVE_TITLE: Color = Color::DarkGray;
const HELP_KEY: Color = Color::Blue;

pub fn draw(f: &mut Frame, app: &mut App) {
    // Main layout: main content | help bar at bottom
    let outer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(f.area());

    // Main content: sidebar | messages
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(40)])
        .split(outer_chunks[0]);

    // Update cached layout areas for mouse detection
    app.set_layout_areas(main_chunks[0], main_chunks[1]);

    draw_channels(f, app, main_chunks[0]);
    draw_messages(f, app, main_chunks[1]);
    draw_status(f, app, outer_chunks[1]);

    // Draw help overlay if active
    if app.show_help() {
        draw_help_overlay(f);
    }
}

fn draw_channels(f: &mut Frame, app: &App, area: Rect) {
    let selected = app.selected_channel_index();
    let public_count = app.channels().len();
    let is_focused = app.focus() == Focus::Channels;

    let mut items: Vec<ListItem> = Vec::new();

    // Public channels
    for (i, ch) in app.channels().iter().enumerate() {
        let new_count = app.new_message_count(ch);
        let is_selected = i == selected;

        let line = format_channel_line(&format!("#{}", ch), new_count, is_selected, is_focused);
        items.push(ListItem::new(line));
    }

    // DM section separator (if there are DMs)
    if !app.dm_channels().is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "── DMs ──",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )])));
    }

    // DM channels - show as Agent1↔Agent2
    for (i, dm) in app.dm_channels().iter().enumerate() {
        let global_idx = public_count + i;
        let display_name = format_dm_channel(dm);
        let new_count = app.new_message_count(dm);
        let is_selected = global_idx == selected;

        let line = format_channel_line_dm(&display_name, new_count, is_selected, is_focused);
        items.push(ListItem::new(line));
    }

    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(ACTIVE_BORDER),
            Style::default().fg(ACTIVE_BORDER),
        )
    } else {
        (Style::default(), Style::default().fg(INACTIVE_TITLE))
    };

    let list = List::new(items).block(
        Block::default()
            .title(Span::styled(" Conversations ", title_style))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style),
    );

    f.render_widget(list, area);
}

/// Format a DM channel name as "Agent1↔Agent2"
fn format_dm_channel(channel: &str) -> String {
    if let Some(agents) = channel.strip_prefix("_dm_") {
        let parts: Vec<&str> = agents.splitn(2, '_').collect();
        if parts.len() == 2 {
            return format!("{}↔{}", parts[0], parts[1]);
        }
    }
    channel.to_string()
}

/// Format a channel line with optional new message count
fn format_channel_line(
    name: &str,
    new_count: usize,
    is_selected: bool,
    is_focused: bool,
) -> Line<'static> {
    let name_style = if is_selected && is_focused {
        Style::default()
            .fg(ACTIVE_BORDER)
            .add_modifier(Modifier::BOLD)
    } else if is_selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else if new_count > 0 {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let mut spans = vec![Span::styled(name.to_string(), name_style)];

    if new_count > 0 {
        spans.push(Span::styled(
            format!(" ({})", new_count),
            Style::default().fg(Color::Yellow),
        ));
    }

    Line::from(spans)
}

/// Format a DM channel line with optional new message count
fn format_channel_line_dm(
    name: &str,
    new_count: usize,
    is_selected: bool,
    is_focused: bool,
) -> Line<'static> {
    let name_style = if is_selected && is_focused {
        Style::default()
            .fg(ACTIVE_BORDER)
            .add_modifier(Modifier::BOLD)
    } else if is_selected {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else if new_count > 0 {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Magenta)
    };

    let mut spans = vec![Span::styled(name.to_string(), name_style)];

    if new_count > 0 {
        spans.push(Span::styled(
            format!(" ({})", new_count),
            Style::default().fg(Color::Yellow),
        ));
    }

    Line::from(spans)
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let raw_channel_name = app
        .current_channel()
        .unwrap_or_else(|| "general".to_string());

    // Format channel names nicely
    let channel_name = if raw_channel_name.starts_with("_dm_") {
        format_dm_channel(&raw_channel_name)
    } else {
        format!("#{}", raw_channel_name)
    };

    let is_focused = app.focus() == Focus::Messages;
    let (border_style, title_style) = if is_focused {
        (
            Style::default().fg(ACTIVE_BORDER),
            Style::default().fg(ACTIVE_BORDER),
        )
    } else {
        (Style::default(), Style::default().fg(INACTIVE_TITLE))
    };

    // Calculate dimensions first
    let inner_width = area.width.saturating_sub(2) as usize; // Account for borders
    let inner_height = area.height.saturating_sub(2) as usize;

    // Convert all messages to lines, inserting separator if needed
    let messages = app.messages();
    let mut lines: Vec<Line> = Vec::new();

    let separator_pos = if app.has_separator(&raw_channel_name) {
        app.separator_position(&raw_channel_name)
    } else {
        None
    };

    for (idx, msg) in messages.iter().enumerate() {
        lines.push(format_message(msg));

        // Insert separator after new messages (which are at the end/bottom)
        if let Some(pos) = separator_pos {
            if idx == pos - 1 {
                lines.push(create_separator_line(inner_width));
            }
        }
    }

    // Estimate wrapped line count - ceiling division for each line
    let total_lines: usize = lines
        .iter()
        .map(|line| {
            let line_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
            if inner_width > 0 && line_len > 0 {
                // Ceiling division: how many lines does this wrap to?
                (line_len + inner_width - 1) / inner_width
            } else {
                1 // Empty line still takes 1 row
            }
        })
        .sum();

    // Clamp scroll to valid range
    let max_scroll = total_lines.saturating_sub(inner_height);
    app.clamp_scroll_lines(max_scroll, inner_height);

    let scroll = app.message_scroll();

    // Use Paragraph's scroll feature - scroll from bottom
    // scroll=0 means show bottom, scroll=max means show top
    let scroll_from_top = max_scroll.saturating_sub(scroll);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(Span::styled(format!(" {} ", channel_name), title_style))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_from_top as u16, 0));

    f.render_widget(paragraph, area);
}

fn create_separator_line(width: usize) -> Line<'static> {
    let text = " New Messages ";
    let text_len = text.len();

    // Calculate padding (leave space for borders)
    let available_width = width.saturating_sub(2);

    if text_len >= available_width {
        // Not enough space, just show the text
        return Line::from(Span::styled(
            text,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Create centered separator with box drawing characters
    let line_char = "─"; // U+2500
    let total_line_len = available_width.saturating_sub(text_len);
    let left_len = total_line_len / 2;
    let right_len = total_line_len - left_len;

    let line_style = Style::default().fg(Color::DarkGray);
    let text_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD);

    Line::from(vec![
        Span::styled(line_char.repeat(left_len), line_style),
        Span::styled(text, text_style),
        Span::styled(line_char.repeat(right_len), line_style),
    ])
}

fn format_message(msg: &crate::core::message::Message) -> Line<'static> {
    let local_time: DateTime<Local> = msg.ts.with_timezone(&Local);
    let time_str = local_time.format("%H:%M").to_string();

    let agent_color = agent_color(&msg.agent);

    let mut spans = vec![
        Span::styled(
            format!("[{}] ", time_str),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{}: ", msg.agent),
            Style::default()
                .fg(agent_color)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    // Add labels if present
    if !msg.labels.is_empty() {
        for label in &msg.labels {
            spans.push(Span::styled(
                format!("[{}] ", label),
                Style::default().fg(Color::Yellow),
            ));
        }
    }

    spans.push(Span::raw(msg.body.clone()));

    // Add attachment indicator if present
    if !msg.attachments.is_empty() {
        spans.push(Span::styled(
            format!(" [{}]", msg.attachments.len()),
            Style::default().fg(Color::Magenta),
        ));
    }

    Line::from(spans)
}

fn agent_color(name: &str) -> Color {
    let hash: usize = name.bytes().map(|b| b as usize).sum();
    let colors = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
    ];
    colors[hash % colors.len()]
}

fn draw_status(f: &mut Frame, _app: &App, area: Rect) {
    let status = Line::from(vec![
        Span::styled(" [Tab] ", Style::default().fg(HELP_KEY)),
        Span::raw("pane  "),
        Span::styled("[j/k] ", Style::default().fg(HELP_KEY)),
        Span::raw("scroll  "),
        Span::styled("[u/d] ", Style::default().fg(HELP_KEY)),
        Span::raw("½page  "),
        Span::styled("[?] ", Style::default().fg(HELP_KEY)),
        Span::raw("help  "),
        Span::styled("[q] ", Style::default().fg(HELP_KEY)),
        Span::raw("quit"),
    ]);

    let paragraph = Paragraph::new(status);
    f.render_widget(paragraph, area);
}

fn draw_help_overlay(f: &mut Frame) {
    let area = f.area();

    // Center a box in the middle of the screen
    let width = 50.min(area.width.saturating_sub(4));
    let height = 18.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    // Clear the area behind the popup
    f.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(vec![Span::styled(
            "Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tab       ", Style::default().fg(HELP_KEY)),
            Span::raw("Switch between panes"),
        ]),
        Line::from(vec![
            Span::styled("  j/k       ", Style::default().fg(HELP_KEY)),
            Span::raw("Scroll down/up (1 line)"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl-j/k  ", Style::default().fg(HELP_KEY)),
            Span::raw("Scroll down/up (10 lines)"),
        ]),
        Line::from(vec![
            Span::styled("  u/d       ", Style::default().fg(HELP_KEY)),
            Span::raw("Scroll up/down (half page)"),
        ]),
        Line::from(vec![
            Span::styled("  b/f       ", Style::default().fg(HELP_KEY)),
            Span::raw("Scroll up/down (full page)"),
        ]),
        Line::from(vec![
            Span::styled("  g/G       ", Style::default().fg(HELP_KEY)),
            Span::raw("Jump to top/bottom"),
        ]),
        Line::from(vec![
            Span::styled("  Enter     ", Style::default().fg(HELP_KEY)),
            Span::raw("Select channel"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Mouse",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Click     ", Style::default().fg(HELP_KEY)),
            Span::raw("Focus pane / select channel"),
        ]),
        Line::from(vec![
            Span::styled("  Wheel     ", Style::default().fg(HELP_KEY)),
            Span::raw("Scroll messages"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(Span::styled(
                    " Keybindings ",
                    Style::default()
                        .fg(ACTIVE_BORDER)
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(ACTIVE_BORDER)),
        )
        .alignment(Alignment::Left);

    f.render_widget(help, popup_area);
}
