//! Sidebar with navigation and playlists.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::{App, Focus, Screen};
use crate::mouse::{Hit, HitMap, Nav};
use crate::ui::{ACCENT, BG, BG_RAISED, BORDER, FG, FG_MUTED, SELECTION};

/// Render the sidebar.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &mut ListState,
    hitmap: &mut HitMap,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            " Library ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 || inner.width == 0 {
        return;
    }

    let library_active = app.screen == Screen::Library && app.focus != Focus::Sidebar;
    let queue_active = app.screen == Screen::Queue;
    render_nav(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        "[L] Library",
        library_active,
        hitmap,
        Nav::Library,
    );
    render_nav(
        frame,
        Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
        &format!("[Q] Queue  {}", app.queue.items.len()),
        queue_active,
        hitmap,
        Nav::Queue,
    );
    if inner.height <= 3 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(usize::from(inner.width)),
            Style::default().fg(FG_MUTED),
        ))),
        Rect::new(inner.x, inner.y.saturating_add(2), inner.width, 1),
    );

    let list_area = Rect::new(
        inner.x,
        inner.y.saturating_add(3),
        inner.width,
        inner.height.saturating_sub(3),
    );
    if list_area.height == 0 {
        return;
    }
    if app.playlists.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No playlists",
                Style::default().fg(FG_MUTED),
            ))),
            list_area,
        );
        return;
    }
    let items = app
        .playlists
        .iter()
        .map(|playlist| ListItem::new(Line::from(playlist.name.clone())))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .style(Style::default().bg(BG).fg(FG))
        .highlight_style(
            Style::default()
                .bg(SELECTION)
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, list_area, state);
    let offset = state.offset();
    let visible = usize::from(list_area.height);
    for index in offset..offset.saturating_add(visible).min(app.playlists.len()) {
        let row = list_area.y.saturating_add((index - offset) as u16);
        hitmap.push(
            Rect::new(list_area.x, row, list_area.width, 1),
            Hit::SidebarPlaylist(index),
        );
    }
}

fn render_nav(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    active: bool,
    hitmap: &mut HitMap,
    nav: Nav,
) {
    let style = if active {
        Style::default()
            .bg(BG_RAISED)
            .fg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG)
    };
    frame.render_widget(
        Paragraph::new(Line::from(label.to_string())).style(style),
        area,
    );
    hitmap.push(area, Hit::Nav(nav));
}
