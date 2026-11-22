//! Main library, playlist and queue views.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState,
};
use recvrd_api::DownloadStatus;

use crate::app::{App, Screen};
use crate::mouse::{Hit, HitMap};
use crate::ui::{ACCENT, BG, BORDER, DANGER, FG, FG_MUTED, SELECTION, SUCCESS, WARNING};

/// Render the main view.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    library_state: &mut TableState,
    playlist_state: &mut TableState,
    queue_state: &mut ListState,
    hitmap: &mut HitMap,
) {
    match app.screen {
        Screen::Library => render_library(frame, area, app, library_state, hitmap),
        Screen::Playlist => render_playlist(frame, area, app, playlist_state, hitmap),
        Screen::Queue => render_queue(frame, area, app, queue_state, hitmap),
    }
}

fn block(title: String) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(BG))
}

fn empty(frame: &mut Frame, area: Rect, message: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(FG_MUTED),
        ))),
        area,
    );
}

fn render_library(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &mut TableState,
    hitmap: &mut HitMap,
) {
    let block = block(format!(
        " Library · {} · {} playlists ",
        app.provider_name(),
        app.playlists.len()
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    if app.playlists.is_empty() {
        empty(frame, inner, "No playlists. Press n to create one.");
        return;
    }
    let rows = app.playlists.iter().map(|playlist| {
        let mut cells = vec![
            Cell::from(playlist.name.clone()),
            Cell::from(playlist.subtitle.clone()).style(Style::default().fg(FG_MUTED)),
        ];
        cells.push(
            Cell::from(
                playlist
                    .item_count
                    .map(|count| count.to_string())
                    .unwrap_or_default(),
            )
            .style(Style::default().fg(FG_MUTED)),
        );
        Row::new(cells)
    });
    let header = Row::new(vec![
        Cell::from("Name"),
        Cell::from("Subtitle"),
        Cell::from("Items"),
    ])
    .style(Style::default().fg(FG_MUTED).add_modifier(Modifier::BOLD));
    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(28),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .style(Style::default().bg(BG).fg(FG))
    .row_highlight_style(
        Style::default()
            .bg(SELECTION)
            .fg(FG)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_stateful_widget(table, inner, state);
    register_table_rows(
        inner,
        app.playlists.len(),
        state.offset(),
        hitmap,
        Hit::LibraryRow,
    );
}

fn render_playlist(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &mut TableState,
    hitmap: &mut HitMap,
) {
    let title = app
        .playlist
        .as_ref()
        .map(|playlist| {
            let mut title = format!(" Playlist · {}", playlist.playlist.name);
            if let Some(author) = &playlist.playlist.author
                && !author.is_empty()
            {
                title.push_str(&format!(" · {author}"));
            }
            if let Some(count) = playlist.playlist.item_count {
                title.push_str(&format!(" · {count} items"));
            }
            format!("{title} ")
        })
        .unwrap_or_else(|| " Playlist ".to_string());
    let block = block(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let Some(playlist) = &app.playlist else {
        empty(frame, inner, "Loading playlist...");
        return;
    };
    if playlist.tracks.is_empty() {
        empty(frame, inner, "This playlist has no tracks.");
        return;
    }
    let wide = inner.width >= 80;
    let rows = playlist.tracks.iter().enumerate().map(|(index, track)| {
        let mut cells = vec![
            Cell::from(format!("{}", index + 1)),
            Cell::from(track.title.clone()),
            Cell::from(track.artist.clone()).style(Style::default().fg(FG_MUTED)),
        ];
        if wide {
            cells.push(Cell::from(track.album.clone()).style(Style::default().fg(FG_MUTED)));
        }
        cells.push(Cell::from(track.duration.clone()).style(Style::default().fg(FG_MUTED)));
        Row::new(cells)
    });
    let mut headers = vec!["#", "Title", "Artist"];
    if wide {
        headers.push("Album");
    }
    headers.push("Duration");
    let header = Row::new(headers.into_iter().map(Cell::from).collect::<Vec<_>>())
        .style(Style::default().fg(FG_MUTED).add_modifier(Modifier::BOLD));
    let widths = if wide {
        vec![
            Constraint::Length(4),
            Constraint::Min(20),
            Constraint::Length(18),
            Constraint::Length(18),
            Constraint::Length(8),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Min(16),
            Constraint::Length(12),
            Constraint::Length(8),
        ]
    };
    let table = Table::new(rows, widths)
        .header(header)
        .style(Style::default().bg(BG).fg(FG))
        .row_highlight_style(
            Style::default()
                .bg(SELECTION)
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(table, inner, state);
    register_table_rows(
        inner,
        playlist.tracks.len(),
        state.offset(),
        hitmap,
        Hit::TrackRow,
    );
}

fn render_queue(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    state: &mut ListState,
    hitmap: &mut HitMap,
) {
    let block = block(format!(" Queue · {} ", app.queue.items.len()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    if app.queue.items.is_empty() {
        empty(frame, inner, "The queue is empty.");
        return;
    }
    let items = app
        .queue
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let marker = if app.queue.current == Some(index) {
                ">"
            } else {
                " "
            };
            let (badge, badge_style): (String, Style) = match &item.download {
                DownloadStatus::NotDownloaded => (".".to_string(), Style::default().fg(FG_MUTED)),
                DownloadStatus::Downloading { progress } => {
                    (format!("v{progress}%"), Style::default().fg(ACCENT))
                }
                DownloadStatus::Downloaded => ("+".to_string(), Style::default().fg(SUCCESS)),
                DownloadStatus::Retrying { .. } => ("~".to_string(), Style::default().fg(WARNING)),
                DownloadStatus::Failed { .. } => ("!".to_string(), Style::default().fg(DANGER)),
            };
            let facts = crate::ui::track_metadata_facts(&item.track);
            let mut detail = item.track.artist.clone();
            if !facts.is_empty() {
                detail.push_str("  ·  ");
                detail.push_str(&facts.join(" · "));
            }
            let line = Line::from(vec![
                Span::styled(
                    format!("{marker} {:>3}  ", index + 1),
                    Style::default().fg(ACCENT),
                ),
                Span::styled(item.track.title.clone(), Style::default().fg(FG)),
                Span::styled(format!("  —  {detail}"), Style::default().fg(FG_MUTED)),
                Span::styled(
                    format!("  {}", item.track.duration),
                    Style::default().fg(FG_MUTED),
                ),
                Span::styled(format!("  {badge}"), badge_style),
            ]);
            ListItem::new(line)
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .style(Style::default().bg(BG).fg(FG))
        .highlight_style(
            Style::default()
                .bg(SELECTION)
                .fg(FG)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, inner, state);
    let offset = state.offset();
    let visible = usize::from(inner.height);
    for index in offset..offset.saturating_add(visible).min(app.queue.items.len()) {
        let row = inner.y.saturating_add((index - offset) as u16);
        hitmap.push(
            Rect::new(inner.x, row, inner.width, 1),
            Hit::QueueRow(index),
        );
    }
}

fn register_table_rows(
    inner: Rect,
    len: usize,
    offset: usize,
    hitmap: &mut HitMap,
    constructor: impl Fn(usize) -> Hit,
) {
    if inner.height <= 1 {
        return;
    }
    let visible = usize::from(inner.height.saturating_sub(1));
    for index in offset..offset.saturating_add(visible).min(len) {
        let row = inner
            .y
            .saturating_add(1)
            .saturating_add((index - offset) as u16);
        hitmap.push(Rect::new(inner.x, row, inner.width, 1), constructor(index));
    }
}
