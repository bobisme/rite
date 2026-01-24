use chrono::{DateTime, Local};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::app::{dm_other_agent, App, Focus};

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

    // Sidebar: channels | agents
    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_chunks[0]);

    // Update cached layout areas for mouse detection
    app.set_layout_areas(sidebar_chunks[0], main_chunks[1]);

    draw_channels(f, app, sidebar_chunks[0]);
    draw_agents(f, app, sidebar_chunks[1]);
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
    let current_agent = app.current_agent();
    let is_focused = app.focus() == Focus::Channels;

    let mut items: Vec<ListItem> = Vec::new();

    // Public channels
    for (i, ch) in app.channels().iter().enumerate() {
        let style = if i == selected && is_focused {
            Style::default()
                .fg(ACTIVE_BORDER)
                .add_modifier(Modifier::BOLD)
        } else if i == selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        items.push(ListItem::new(format!("#{}", ch)).style(style));
    }

    // DM section separator (if there are DMs)
    if !app.dm_channels().is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "-- DMs --",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )])));
    }

    // DM channels
    for (i, dm) in app.dm_channels().iter().enumerate() {
        let global_idx = public_count + i;
        let display_name = if let Some(agent) = current_agent {
            dm_other_agent(dm, agent).unwrap_or_else(|| dm.clone())
        } else {
            dm.clone()
        };

        let style = if global_idx == selected && is_focused {
            Style::default()
                .fg(ACTIVE_BORDER)
                .add_modifier(Modifier::BOLD)
        } else if global_idx == selected {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Magenta)
        };
        items.push(ListItem::new(format!("@{}", display_name)).style(style));
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
            .title(Span::styled(" Channels ", title_style))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style),
    );

    f.render_widget(list, area);
}

fn draw_agents(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .agents()
        .iter()
        .map(|agent| {
            let style = if Some(agent.name.as_str()) == app.current_agent() {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("  {}", agent.name)).style(style)
        })
        .collect();

    // Agents pane is never focused, always show inactive style
    let list = List::new(items).block(
        Block::default()
            .title(Span::styled(
                " Agents ",
                Style::default().fg(INACTIVE_TITLE),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );

    f.render_widget(list, area);
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let raw_channel_name = app
        .current_channel()
        .unwrap_or_else(|| "general".to_string());

    // Format DM channel names nicely
    let channel_name = if raw_channel_name.starts_with("_dm_") {
        if let Some(agent) = app.current_agent() {
            if let Some(other) = dm_other_agent(&raw_channel_name, agent) {
                format!("@{}", other)
            } else {
                raw_channel_name
            }
        } else {
            raw_channel_name
        }
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

    // Convert all messages to lines
    let messages = app.messages();
    let lines: Vec<Line> = messages.iter().map(|msg| format_message(msg)).collect();

    // Calculate the total rendered height accounting for wrapping
    let inner_width = area.width.saturating_sub(2) as usize; // Account for borders
    let inner_height = area.height.saturating_sub(2) as usize;

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
