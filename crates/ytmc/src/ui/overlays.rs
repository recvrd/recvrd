//! Search, help, context menus, dialogs and toasts.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};

use crate::app::{App, Overlay, SearchEntry};
use crate::event::ToastLevel;
use crate::mouse::{Hit, HitMap};
use crate::ui::{
    ACCENT, ACCENT_2, BG_RAISED, BORDER, DANGER, FG, FG_MUTED, SELECTION, SUCCESS, centered,
};

/// Render all overlays and toasts.
pub fn render(frame: &mut Frame, area: Rect, app: &App, hitmap: &mut HitMap) {
    match &app.overlay {
        Some(Overlay::Help) => render_help(frame, area, hitmap),
        Some(Overlay::Search) => render_search(frame, area, app, hitmap),
        Some(Overlay::Context(context)) => render_context(frame, area, context, hitmap),
        Some(Overlay::Input(input)) => render_input(frame, area, input, hitmap),
        Some(Overlay::Confirm(confirm)) => render_confirm(frame, area, confirm, hitmap),
        Some(Overlay::PickPlaylist(picker)) => {
            render_picker(frame, area, app, picker, hitmap);
        }
        None => {}
    }
    render_toasts(frame, area, app, hitmap);
}

fn overlay_block(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            title.into(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG_RAISED))
}

fn render_search(frame: &mut Frame, area: Rect, app: &App, hitmap: &mut HitMap) {
    let overlay = centered(
        area,
        (area.width * 8 / 10).max(40),
        (area.height * 8 / 10).max(10),
    );
    let title = if app.search.loading {
        " Search · loading "
    } else {
        " Search "
    };
    let block = overlay_block(title);
    let inner = block.inner(overlay);
    frame.render_widget(Clear, overlay);
    frame.render_widget(block, overlay);
    hitmap.push(overlay, Hit::Overlay);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let input = Line::from(vec![
        Span::styled("> ", Style::default().fg(ACCENT)),
        Span::styled(app.search.input.clone(), Style::default().fg(FG)),
    ]);
    frame.render_widget(
        Paragraph::new(input),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height < 3 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(usize::from(inner.width)),
            Style::default().fg(FG_MUTED),
        ))),
        Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
    );

    let results_area = Rect::new(
        inner.x,
        inner.y.saturating_add(2),
        inner.width,
        inner.height.saturating_sub(3),
    );
    let entries = app.search.entries();
    if entries.is_empty() {
        let message = if app.search.query.is_empty() {
            "Type to search."
        } else if app.search.loading {
            "Searching..."
        } else {
            "No results."
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message,
                Style::default().fg(FG_MUTED),
            ))),
            results_area,
        );
    } else if results_area.height > 0 {
        let visible = usize::from(results_area.height);
        let selected = app.search.selected.min(entries.len() - 1);
        let mut scroll = selected.saturating_sub(visible.saturating_sub(1));
        if selected < scroll {
            scroll = selected;
        }
        if selected >= scroll + visible {
            scroll = selected + 1 - visible;
        }
        for (index, entry) in entries.iter().enumerate().skip(scroll).take(visible) {
            let (prefix, title, subtitle, color) = match entry {
                SearchEntry::Track(track) => (
                    "♪",
                    track.title.clone(),
                    format!("{} · {} · {}", track.artist, track.album, track.duration),
                    FG,
                ),
                SearchEntry::Playlist(playlist) => (
                    "≡",
                    playlist.name.clone(),
                    playlist.subtitle.clone(),
                    ACCENT_2,
                ),
            };
            let style = if index == selected {
                Style::default()
                    .bg(SELECTION)
                    .fg(FG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FG)
            };
            let line = Line::from(vec![
                Span::styled(format!(" {prefix} "), Style::default().fg(color)),
                Span::styled(title, style),
                Span::styled(format!("  {subtitle}"), Style::default().fg(FG_MUTED)),
            ]);
            let row = results_area.y.saturating_add((index - scroll) as u16);
            let rect = Rect::new(results_area.x, row, results_area.width, 1);
            frame.render_widget(Paragraph::new(line).style(style), rect);
            hitmap.push(rect, Hit::SearchRow(index));
        }
    }
    let footer = Rect::new(
        inner.x,
        inner.y.saturating_add(inner.height.saturating_sub(1)),
        inner.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Enter play/open · Ctrl+X queue · Ctrl+A add · Esc close",
            Style::default().fg(FG_MUTED),
        ))),
        footer,
    );
}

fn render_help(frame: &mut Frame, area: Rect, hitmap: &mut HitMap) {
    let overlay = centered(area, 64.min(area.width), 24.min(area.height));
    let block = overlay_block(" Keys and mouse ");
    let inner = block.inner(overlay);
    frame.render_widget(Clear, overlay);
    frame.render_widget(block, overlay);
    hitmap.push(overlay, Hit::Overlay);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let rows = [
        ("j / Down", "Move down"),
        ("k / Up", "Move up"),
        ("gg / G", "First / last"),
        ("Ctrl+D / Ctrl+U", "Half page down / up"),
        ("Enter", "Play or open"),
        ("Space", "Play or pause"),
        ("x / a", "Queue / add to playlist"),
        ("dd", "Delete"),
        ("D / t", "Clear queue / retry download"),
        ("J / K", "Move item"),
        ("n / r", "New / rename playlist"),
        ("p", "Play playlist"),
        ("s / R", "Shuffle / repeat"),
        ("+ / -", "Volume"),
        ("Left / Right", "Seek 5 seconds"),
        ("Ctrl+Left / Ctrl+Right", "Previous / next track"),
        ("/ or Ctrl+K", "Search"),
        ("Ctrl+R", "Reload library"),
        ("1 / 2 / Tab", "Library, queue, focus"),
        ("? / q / Ctrl+C", "Help / quit"),
        ("Click / double click", "Select / play"),
        ("Right click", "Open the track menu"),
    ];
    let lines = rows
        .iter()
        .map(|(key, action)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key:<22}"),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled((*action).to_string(), Style::default().fg(FG)),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(Style::default().fg(FG)), inner);
}

fn render_context(
    frame: &mut Frame,
    area: Rect,
    context: &crate::app::ContextMenu,
    hitmap: &mut HitMap,
) {
    let width = 32.min(area.width);
    let height = (context.items.len() as u16 + 2).min(area.height);
    if width == 0 || height == 0 {
        return;
    }
    let x = context
        .x
        .min(area.x.saturating_add(area.width.saturating_sub(width)));
    let y = context
        .y
        .min(area.y.saturating_add(area.height.saturating_sub(height)));
    let overlay = Rect::new(x, y, width, height);
    let block = overlay_block(context.title.clone());
    let inner = block.inner(overlay);
    frame.render_widget(Clear, overlay);
    frame.render_widget(block, overlay);
    hitmap.push(overlay, Hit::Overlay);
    for (index, item) in context.items.iter().enumerate() {
        let row = inner.y.saturating_add(index as u16);
        if row >= inner.y.saturating_add(inner.height) {
            break;
        }
        let style = if index == context.selected {
            Style::default()
                .bg(SELECTION)
                .fg(FG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG)
        };
        let rect = Rect::new(inner.x, row, inner.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(format!(" {}", item.label))).style(style),
            rect,
        );
        hitmap.push(rect, Hit::ContextRow(index));
    }
}

fn render_input(
    frame: &mut Frame,
    area: Rect,
    input: &crate::app::InputModal,
    hitmap: &mut HitMap,
) {
    let overlay = centered(area, 50.min(area.width), 5.min(area.height));
    let block = overlay_block(input.title.clone());
    let inner = block.inner(overlay);
    frame.render_widget(Clear, overlay);
    frame.render_widget(block, overlay);
    hitmap.push(overlay, Hit::Overlay);
    if inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::styled(input.value.clone(), Style::default().fg(FG)),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height > 1 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Enter save · Esc cancel",
                Style::default().fg(FG_MUTED),
            ))),
            Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
        );
    }
}

fn render_confirm(
    frame: &mut Frame,
    area: Rect,
    confirm: &crate::app::ConfirmModal,
    hitmap: &mut HitMap,
) {
    let overlay = centered(area, 50.min(area.width), 7.min(area.height));
    let block = overlay_block(confirm.title.clone());
    let inner = block.inner(overlay);
    frame.render_widget(Clear, overlay);
    frame.render_widget(block, overlay);
    hitmap.push(overlay, Hit::Overlay);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            confirm.message.clone(),
            Style::default().fg(FG),
        ))),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if inner.height > 2 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[y] Yes", Style::default().fg(DANGER)),
                Span::raw("   "),
                Span::styled("[n] No", Style::default().fg(FG_MUTED)),
            ])),
            Rect::new(inner.x, inner.y.saturating_add(2), inner.width, 1),
        );
    }
}

fn render_picker(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    picker: &crate::app::PickPlaylistModal,
    hitmap: &mut HitMap,
) {
    let overlay = centered(area, 50.min(area.width), (area.height * 6 / 10).max(7));
    let block = overlay_block(" Add to playlist ");
    let inner = block.inner(overlay);
    frame.render_widget(Clear, overlay);
    frame.render_widget(block, overlay);
    hitmap.push(overlay, Hit::Overlay);
    if inner.height == 0 {
        return;
    }
    if app.playlists.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No playlists.",
                Style::default().fg(FG_MUTED),
            ))),
            inner,
        );
        return;
    }
    let items = app
        .playlists
        .iter()
        .map(|playlist| ListItem::new(Line::from(playlist.name.clone())))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .style(Style::default().bg(BG_RAISED).fg(FG))
        .highlight_style(
            Style::default()
                .bg(SELECTION)
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(picker.selected.min(app.playlists.len() - 1)));
    frame.render_stateful_widget(list, inner, &mut state);
    let offset = state.offset();
    let visible = usize::from(inner.height);
    for index in offset..offset.saturating_add(visible).min(app.playlists.len()) {
        let row = inner.y.saturating_add((index - offset) as u16);
        hitmap.push(
            Rect::new(inner.x, row, inner.width, 1),
            Hit::PlaylistPickerRow(index),
        );
    }
}

fn render_toasts(frame: &mut Frame, area: Rect, app: &App, hitmap: &mut HitMap) {
    let width = 42.min(area.width);
    if width == 0 || area.height < 4 {
        return;
    }
    let mut bottom = area.y.saturating_add(area.height).saturating_sub(2);
    for (index, toast) in app.toasts.iter().enumerate().rev().take(3) {
        let height = 3.min(bottom.saturating_sub(area.y));
        if height == 0 {
            break;
        }
        let y = bottom.saturating_sub(height);
        let rect = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width)),
            y,
            width,
            height,
        );
        let (color, title) = match toast.level {
            ToastLevel::Info => (ACCENT_2, " Info "),
            ToastLevel::Success => (SUCCESS, " Done "),
            ToastLevel::Error => (DANGER, " Error "),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color))
            .title(Span::styled(
                title,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(BG_RAISED));
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                toast.message.clone(),
                Style::default().fg(FG),
            )))
            .alignment(Alignment::Left),
            inner,
        );
        hitmap.push(rect, Hit::Toast(index));
        bottom = y.saturating_sub(1);
    }
}
