//! Rendering for the terminal UI.

mod main_view;
mod now_playing;
mod overlays;
mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, ListState, Paragraph, TableState};

use crate::app::{App, Connection, Overlay};
use crate::mouse::HitMap;
use recvrd_api::DownloadStatus;

/// Main background color from the web UI theme.
pub const BG: Color = Color::Rgb(9, 24, 51);
/// Raised background color.
pub const BG_RAISED: Color = Color::Rgb(13, 36, 71);
/// Sunken background color.
pub const BG_SUNKEN: Color = Color::Rgb(6, 18, 42);
/// Primary text color.
pub const FG: Color = Color::Rgb(227, 232, 252);
/// Muted text color.
pub const FG_MUTED: Color = Color::Rgb(143, 163, 200);
/// Accent color.
pub const ACCENT: Color = Color::Rgb(234, 0, 217);
/// Secondary accent color.
pub const ACCENT_2: Color = Color::Rgb(10, 189, 198);
/// Danger color.
pub const DANGER: Color = Color::Rgb(255, 77, 109);
/// Warning color.
pub const WARNING: Color = Color::Rgb(229, 192, 123);
/// Success color.
pub const SUCCESS: Color = Color::Rgb(76, 217, 100);
/// Selection background color.
pub const SELECTION: Color = Color::Rgb(19, 62, 124);
/// Border color.
pub const BORDER: Color = ACCENT_2;

/// Layout areas for one frame.
pub struct Areas {
    /// Header line.
    pub header: Rect,
    /// Warning banner.
    pub banner: Rect,
    /// Sidebar.
    pub sidebar: Rect,
    /// Main view.
    pub main: Rect,
    /// Now-playing bar.
    pub now_playing: Rect,
    /// Status line.
    pub status: Rect,
}

/// Compute the frame areas.
pub fn layout(area: Rect, banner: bool, sidebar: bool) -> Areas {
    let banner_height = u16::from(banner && area.height > 9);
    let now_height = if area.height >= 12 {
        6
    } else {
        4.min(area.height)
    };
    let status_height = 1.min(area.height);
    let body_height = area
        .height
        .saturating_sub(1 + banner_height + now_height + status_height);
    let header = Rect::new(area.x, area.y, area.width, 1.min(area.height));
    let banner_rect = Rect::new(area.x, area.y.saturating_add(1), area.width, banner_height);
    let body = Rect::new(
        area.x,
        area.y.saturating_add(1 + banner_height),
        area.width,
        body_height,
    );
    let now_playing = Rect::new(
        area.x,
        body.y.saturating_add(body.height),
        area.width,
        now_height,
    );
    let status = Rect::new(
        area.x,
        now_playing.y.saturating_add(now_playing.height),
        area.width,
        status_height,
    );
    let (sidebar_rect, main_rect) = if sidebar && body.width >= 60 {
        let chunks = Layout::horizontal([Constraint::Length(24), Constraint::Min(20)]).split(body);
        (chunks[0], chunks[1])
    } else {
        (Rect::default(), body)
    };
    Areas {
        header,
        banner: banner_rect,
        sidebar: sidebar_rect,
        main: main_rect,
        now_playing,
        status,
    }
}

/// Render one frame.
pub fn render(frame: &mut Frame, app: &mut App) {
    let mut library_state = std::mem::take(&mut app.library_state);
    let mut playlist_state = std::mem::take(&mut app.playlist_state);
    let mut queue_state = std::mem::take(&mut app.queue_state);
    let mut sidebar_state = std::mem::take(&mut app.sidebar_state);
    let mut hitmap = HitMap::default();
    render_frame(
        frame,
        app,
        &mut library_state,
        &mut playlist_state,
        &mut queue_state,
        &mut sidebar_state,
        &mut hitmap,
    );
    app.library_state = library_state;
    app.playlist_state = playlist_state;
    app.queue_state = queue_state;
    app.sidebar_state = sidebar_state;
    app.hitmap = hitmap;
}

fn render_frame(
    frame: &mut Frame,
    app: &mut App,
    library_state: &mut TableState,
    playlist_state: &mut TableState,
    queue_state: &mut ListState,
    sidebar_state: &mut ListState,
    hitmap: &mut HitMap,
) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    let banner = app.connection == Connection::Offline || app.auth_problem().is_some();
    let areas = layout(area, banner, true);

    render_header(frame, areas.header, app);
    if areas.banner.height > 0 {
        render_banner(frame, areas.banner, app);
    }
    if areas.sidebar.width > 0 {
        hitmap.push(areas.sidebar, crate::mouse::Hit::SidebarPane);
        sidebar::render(frame, areas.sidebar, app, sidebar_state, hitmap);
    }
    if areas.main.height > 0 {
        hitmap.push(areas.main, crate::mouse::Hit::MainPane);
        main_view::render(
            frame,
            areas.main,
            app,
            library_state,
            playlist_state,
            queue_state,
            hitmap,
        );
    }
    if areas.now_playing.height > 0 {
        now_playing::render(frame, areas.now_playing, app, hitmap);
    }
    if areas.status.height > 0 {
        render_status(frame, areas.status, app);
    }
    overlays::render(frame, area, app, hitmap);
    set_cursor(frame, area, app);
}

/// The provider metadata that fits in one compact line. Empty when
/// the provider supplies no metadata or repeats the album.
pub fn track_metadata_facts(track: &recvrd_api::Track) -> Vec<String> {
    let Some(metadata) = &track.metadata else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    if let Some(channel) = &metadata.channel_title
        && *channel != track.album
    {
        facts.push(channel.clone());
    }
    match (metadata.season_number, metadata.episode_number) {
        (Some(season), Some(episode)) => facts.push(format!("S{season}E{episode}")),
        (Some(season), None) => facts.push(format!("S{season}")),
        (None, Some(episode)) => facts.push(format!("E{episode}")),
        (None, None) => {}
    }
    if let Some(year) = metadata.year {
        facts.push(year.to_string());
    }
    if let Some(genre) = &metadata.genre {
        facts.push(genre.clone());
    }
    facts
}

/// Build the header line for the current app state.
fn header_spans(app: &App) -> Vec<Span<'static>> {
    let (label, color) = match app.connection {
        Connection::Connecting => ("connecting", WARNING),
        Connection::Online => ("online", SUCCESS),
        Connection::Offline => ("offline", DANGER),
    };
    let mut spans = vec![
        Span::styled(
            " recvc ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("· ", Style::default().fg(FG_MUTED)),
        Span::styled(app.url.clone(), Style::default().fg(FG_MUTED)),
        Span::styled(" · ", Style::default().fg(FG_MUTED)),
        Span::styled(app.provider_name(), Style::default().fg(ACCENT_2)),
        Span::styled(" · ", Style::default().fg(FG_MUTED)),
        Span::styled(label, Style::default().fg(color)),
    ];
    if let Some(mode) = app
        .providers
        .iter()
        .find(|provider| provider.id == app.provider)
        .map(|provider| match provider.playback {
            recvrd_api::PlaybackMode::Download => "download",
            recvrd_api::PlaybackMode::Stream => "stream",
            recvrd_api::PlaybackMode::StreamThenDownload => "stream+cache",
        })
    {
        spans.push(Span::styled(" · ", Style::default().fg(FG_MUTED)));
        spans.push(Span::styled(mode, Style::default().fg(ACCENT_2)));
    }
    if app.auth_problem().is_some() {
        spans.push(Span::styled(" · auth error", Style::default().fg(DANGER)));
    }
    spans
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(Line::from(header_spans(app))).style(Style::default().bg(BG_RAISED)),
        area,
    );
}

fn render_banner(frame: &mut Frame, area: Rect, app: &App) {
    let message = if app.connection == Connection::Offline {
        app.connection_message
            .clone()
            .unwrap_or_else(|| "The daemon is offline. Retrying.".to_string())
    } else {
        app.auth_problem()
            .map(|message| format!("Auth: {message}"))
            .unwrap_or_default()
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {message}"),
            Style::default().fg(WARNING),
        )))
        .style(Style::default().bg(BG_SUNKEN)),
        area,
    );
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let (queued, active, _, _, failed) = download_counts(app);
    let downloads = format!("{queued} queued · {active} active · {failed} failed");
    let hint = if matches!(app.overlay, Some(Overlay::Search)) {
        "Type to search · Enter to submit · Esc to close"
    } else {
        "Ctrl+C quit · ? help · / search · Tab focus"
    };
    let line = Line::from(vec![
        Span::styled(format!(" {hint}"), Style::default().fg(FG_MUTED)),
        Span::styled(
            format!("  │  queue {}  │  {downloads}", app.queue.items.len()),
            Style::default().fg(FG_MUTED),
        ),
        Span::styled(
            app.status
                .as_ref()
                .and_then(|status| status.playback.source_kind)
                .map(|kind| {
                    format!(
                        "  │  source {}",
                        match kind {
                            recvrd_api::PlaybackSourceKind::File => "cache",
                            recvrd_api::PlaybackSourceKind::Stream => "stream",
                        }
                    )
                })
                .unwrap_or_default(),
            Style::default().fg(ACCENT_2),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(BG_RAISED).fg(FG)),
        area,
    );
}

fn set_cursor(frame: &mut Frame, area: Rect, app: &App) {
    let mut position = None;
    match &app.overlay {
        Some(Overlay::Search) => {
            let overlay = centered(
                area,
                area.width.saturating_mul(8) / 10,
                area.height.saturating_mul(8) / 10,
            );
            let inner = Block::bordered().inner(overlay);
            if inner.height > 0 {
                position = Some(Position::new(
                    inner
                        .x
                        .saturating_add(2 + app.search.input.chars().count() as u16),
                    inner.y,
                ));
            }
        }
        Some(Overlay::Input(input)) => {
            let overlay = centered(area, 50.min(area.width), 5.min(area.height));
            let inner = Block::bordered().inner(overlay);
            if inner.height > 0 {
                position = Some(Position::new(
                    inner
                        .x
                        .saturating_add(2 + input.value.chars().count() as u16),
                    inner.y,
                ));
            }
        }
        _ => {}
    }
    if let Some(position) = position {
        frame.set_cursor_position(position);
    }
}

/// Return queue download counts: queued, active, completed, retrying, failed.
pub fn download_counts(app: &App) -> (usize, usize, usize, usize, usize) {
    let mut counts = (0, 0, 0, 0, 0);
    for item in &app.queue.items {
        match item.download {
            DownloadStatus::NotDownloaded => counts.0 += 1,
            DownloadStatus::Downloading { .. } => counts.1 += 1,
            DownloadStatus::Downloaded => counts.2 += 1,
            DownloadStatus::Retrying { .. } => counts.3 += 1,
            DownloadStatus::Failed { .. } => counts.4 += 1,
        }
    }
    counts
}

/// Return a centered rectangle with the given size.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Format seconds as `M:SS` or `H:MM:SS`.
pub fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use recvrd_api::{
        AuthState, AuthStatus, DownloadStatus, DownloadSummary, PlaybackSnapshot, PlaybackState,
        Playlist, ProviderId, Queue, QueueItem, RepeatMode, Status, Track,
    };

    fn track() -> Track {
        Track {
            provider: ProviderId::Youtube,
            track_id: "v1".to_string(),
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            ..Default::default()
        }
    }

    fn app() -> App {
        let mut app = App::new("http://127.0.0.1:8080");
        app.connection = Connection::Online;
        app.status = Some(Status {
            name: "recvrd".to_string(),
            version: "0.2.0".to_string(),
            status: "ok".to_string(),
            uptime_seconds: 1,
            auth: AuthStatus {
                state: AuthState::Ok,
                source: None,
                message: None,
            },
            playback: PlaybackSnapshot {
                state: PlaybackState::Playing,
                current: Some(track()),
                position_secs: 12.0,
                volume: 50,
                repeat: RepeatMode::Off,
                shuffle: false,
                source_kind: None,
                offline: false,
            },
            queue_length: 1,
            downloads: DownloadSummary::default(),
            providers: vec![ProviderId::Youtube],
            default_provider: ProviderId::Youtube,
        });
        app.queue = Queue {
            items: vec![QueueItem {
                track: track(),
                download: DownloadStatus::Downloaded,
            }],
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
        };
        app.playlists = vec![Playlist {
            id: "p1".to_string(),
            name: "Playlist".to_string(),
            subtitle: "Artist".to_string(),
            ..Default::default()
        }];
        app.sync_selection();
        app
    }

    fn render_to_text(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for row in 0..height {
            for column in 0..width {
                text.push_str(buffer[(column, row)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn renders_the_library_at_80_by_24() {
        let mut app = app();
        let text = render_to_text(&mut app, 80, 24);
        assert!(text.contains("recvc"));
        assert!(text.contains("Library"));
        assert!(text.contains("Playlist"));
        assert!(text.contains("Song"));
        assert!(app.hitmap.hit(1, 1).is_some());
    }

    #[test]
    fn renders_the_queue_and_search_overlay() {
        let mut app = app();
        app.screen = crate::app::Screen::Queue;
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("Queue"));
        app.overlay = Some(Overlay::Search);
        app.search.input = "song".to_string();
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("Search"));
        assert!(text.contains("song"));
    }

    #[test]
    fn renders_the_active_provider_in_the_queue_view() {
        let mut app = app();
        app.provider = ProviderId::Rss;
        app.screen = crate::app::Screen::Queue;
        let text = render_to_text(&mut app, 120, 40);
        assert!(text.contains("RSS"), "{text}");
    }

    #[test]
    fn renders_help_and_a_small_terminal() {
        let mut app = app();
        app.overlay = Some(Overlay::Help);
        let text = render_to_text(&mut app, 60, 20);
        assert!(text.contains("Keys"));
        let text = render_to_text(&mut app, 40, 10);
        assert!(text.contains("Move down"));
    }

    #[test]
    fn formats_times() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(204.0), "3:24");
        assert_eq!(format_time(3723.0), "1:02:03");
    }
}
