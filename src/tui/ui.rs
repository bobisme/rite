use chrono::{DateTime, Local};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::app::{dm_other_agent, App, Focus};

pub fn draw(f: &mut Frame, app: &App) {
    // Main layout: sidebar | content
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(20), Constraint::Min(40)])
        .split(f.area());

    // Sidebar: channels | agents
    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(main_chunks[0]);

    // Content: messages | status
    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(main_chunks[1]);

    draw_channels(f, app, sidebar_chunks[0]);
    draw_agents(f, app, sidebar_chunks[1]);
    draw_messages(f, app, content_chunks[0]);
    draw_status(f, app, content_chunks[1]);
}

fn draw_channels(f: &mut Frame, app: &App, area: Rect) {
    let selected = app.selected_channel_index();
    let public_count = app.channels().len();
    let current_agent = app.current_agent();

    let mut items: Vec<ListItem> = Vec::new();

    // Public channels
    for (i, ch) in app.channels().iter().enumerate() {
        let style = if i == selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
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
        // Account for the separator line
        let display_name = if let Some(agent) = current_agent {
            dm_other_agent(dm, agent).unwrap_or_else(|| dm.clone())
        } else {
            dm.clone()
        };

        let style = if global_idx == selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Magenta)
        };
        items.push(ListItem::new(format!("@{}", display_name)).style(style));
    }

    let border_style = if app.focus() == Focus::Channels {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let list = List::new(items).block(
        Block::default()
            .title(" Channels ")
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

    let list = List::new(items).block(
        Block::default()
            .title(" Agents ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );

    f.render_widget(list, area);
}

fn draw_messages(f: &mut Frame, app: &App, area: Rect) {
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

    let border_style = if app.focus() == Focus::Messages {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    // Calculate visible messages
    let inner_height = area.height.saturating_sub(2) as usize;
    let messages = app.messages();
    let scroll = app.message_scroll();

    // Clamp scroll so viewport doesn't shrink when scrolled past the beginning
    let max_scroll = messages.len().saturating_sub(inner_height);
    let clamped_scroll = scroll.min(max_scroll);

    let start = messages.len().saturating_sub(inner_height + clamped_scroll);
    let end = messages.len().saturating_sub(clamped_scroll);
    let visible: Vec<_> = messages.get(start..end).unwrap_or(&[]).to_vec();

    let lines: Vec<Line> = visible.iter().map(|msg| format_message(msg)).collect();

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" {} ", channel_name))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false });

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
        Span::styled(" [Tab] ", Style::default().fg(Color::Cyan)),
        Span::raw("pane  "),
        Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
        Span::raw("scroll  "),
        Span::styled("[g/G] ", Style::default().fg(Color::Cyan)),
        Span::raw("top/bottom  "),
        Span::styled("[q] ", Style::default().fg(Color::Cyan)),
        Span::raw("quit"),
    ]);

    let paragraph = Paragraph::new(status);
    f.render_widget(paragraph, area);
}
