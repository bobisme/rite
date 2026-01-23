use chrono::{DateTime, Local};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::app::{App, Focus};

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

    // Content: messages | input | status
    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(main_chunks[1]);

    draw_channels(f, app, sidebar_chunks[0]);
    draw_agents(f, app, sidebar_chunks[1]);
    draw_messages(f, app, content_chunks[0]);
    draw_input(f, app, content_chunks[1]);
    draw_status(f, app, content_chunks[2]);
}

fn draw_channels(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .channels()
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let style = if i == app.selected_channel_index() {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("#{}", ch)).style(style)
        })
        .collect();

    let border_style = if app.focus() == Focus::Channels {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let list = List::new(items).block(
        Block::default()
            .title(" Channels ")
            .borders(Borders::ALL)
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

    let list = List::new(items).block(Block::default().title(" Agents ").borders(Borders::ALL));

    f.render_widget(list, area);
}

fn draw_messages(f: &mut Frame, app: &App, area: Rect) {
    let channel_name = app
        .current_channel()
        .unwrap_or_else(|| "general".to_string());

    let border_style = if app.focus() == Focus::Messages {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    // Calculate visible messages
    let inner_height = area.height.saturating_sub(2) as usize;
    let messages = app.messages();
    let scroll = app.message_scroll();

    let start = messages.len().saturating_sub(inner_height + scroll);
    let end = messages.len().saturating_sub(scroll);
    let visible: Vec<_> = messages.get(start..end).unwrap_or(&[]).to_vec();

    let lines: Vec<Line> = visible.iter().map(|msg| format_message(msg)).collect();

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" #{} ", channel_name))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

fn format_message(msg: &crate::core::message::Message) -> Line<'static> {
    let local_time: DateTime<Local> = msg.ts.with_timezone(&Local);
    let time_str = local_time.format("%H:%M").to_string();

    let agent_color = agent_color(&msg.agent);

    Line::from(vec![
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
        Span::raw(msg.body.clone()),
    ])
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

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus() == Focus::Input {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };

    let input = Paragraph::new(app.input()).block(
        Block::default()
            .title(" Message ")
            .borders(Borders::ALL)
            .border_style(border_style),
    );

    f.render_widget(input, area);

    // Show cursor in input mode
    if app.focus() == Focus::Input {
        f.set_cursor_position((area.x + app.input().len() as u16 + 1, area.y + 1));
    }
}

fn draw_status(f: &mut Frame, _app: &App, area: Rect) {
    let status = Line::from(vec![
        Span::styled(" [Tab] ", Style::default().fg(Color::Cyan)),
        Span::raw("pane  "),
        Span::styled("[Enter] ", Style::default().fg(Color::Cyan)),
        Span::raw("send  "),
        Span::styled("[j/k] ", Style::default().fg(Color::Cyan)),
        Span::raw("scroll  "),
        Span::styled("[q] ", Style::default().fg(Color::Cyan)),
        Span::raw("quit"),
    ]);

    let paragraph = Paragraph::new(status);
    f.render_widget(paragraph, area);
}
