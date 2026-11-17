//! Now-playing bar with cover art, progress and controls.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, LineGauge, Paragraph};

use crate::app::App;
use crate::cover::CoverArt;
use crate::mouse::{Hit, HitMap};
use crate::ui::{
    ACCENT, ACCENT_2, BG, BG_RAISED, BORDER, FG, FG_MUTED, SELECTION, WARNING, download_counts,
    format_time,
};

/// Render the now-playing bar.
pub fn render(frame: &mut Frame, area: Rect, app: &App, hitmap: &mut HitMap) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG_RAISED));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let has_cover = inner.width >= 50;
    let chunks = if has_cover {
        Layout::horizontal([
            Constraint::Length(12),
            Constraint::Min(20),
            Constraint::Length(28),
        ])
        .split(inner)
    } else {
        Layout::horizontal([
            Constraint::Length(0),
            Constraint::Min(20),
            Constraint::Length(24),
        ])
        .split(inner)
    };
    if has_cover {
        render_cover_area(frame, chunks[0], app);
    }
    render_info(frame, chunks[1], app, hitmap);
    render_controls(frame, chunks[2], app, hitmap);
}

fn render_cover_area(frame: &mut Frame, area: Rect, app: &App) {
    let current = app.current_track();
    if let (Some(track), Some(art)) = (current, app.cover.as_ref())
        && app.cover_for.as_ref() == Some(&track.key())
    {
        render_cover(frame, area, art);
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "♪",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center),
        inner,
    );
}

fn render_cover(frame: &mut Frame, area: Rect, art: &CoverArt) {
    let height = usize::from(art.height).min(usize::from(area.height));
    let width = usize::from(art.width).min(usize::from(area.width));
    let x = area
        .x
        .saturating_add((area.width.saturating_sub(width as u16)) / 2);
    let area = Rect::new(x, area.y, width as u16, height as u16);
    let mut lines = Vec::with_capacity(height);
    for row in 0..height {
        let cells = art.row(row);
        let spans = cells
            .iter()
            .take(width)
            .map(|cell| Span::styled("▀", Style::default().fg(cell.top).bg(cell.bottom)))
            .collect::<Vec<_>>();
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_info(frame: &mut Frame, area: Rect, app: &App, hitmap: &mut HitMap) {
    let track = app.current_track();
    let title = track
        .as_ref()
        .map(|track| track.title.clone())
        .unwrap_or_else(|| "Nothing playing".to_string());
    let subtitle = track
        .as_ref()
        .map(|track| format!("{} · {}", track.artist, track.album))
        .unwrap_or_else(|| "Start a playlist or press / to search.".to_string());
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().fg(FG).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if area.height > 1 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                subtitle,
                Style::default().fg(FG_MUTED),
            ))),
            Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
        );
    }
    if area.height > 2 {
        let position = app.position_secs();
        let duration = app.duration_secs().unwrap_or(0.0);
        let ratio = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let label = format!(
            "{} / {}",
            format_time(position),
            app.duration_secs()
                .map(format_time)
                .unwrap_or_else(|| "--:--".to_string())
        );
        let gauge_area = Rect::new(area.x, area.y.saturating_add(2), area.width, 1);
        let gauge = LineGauge::default()
            .ratio(ratio)
            .label(Line::from(label))
            .filled_style(Style::default().fg(ACCENT))
            .unfilled_style(Style::default().fg(BG));
        frame.render_widget(gauge, gauge_area);
        hitmap.push(gauge_area, Hit::Progress);
    }
    if area.height > 3 {
        let hint = if track.is_some() {
            "Space play/pause · ←/→ seek · +/- volume"
        } else {
            "Ctrl+C quit · ? help"
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(FG_MUTED),
            ))),
            Rect::new(area.x, area.y.saturating_add(3), area.width, 1),
        );
    }
}

fn render_controls(frame: &mut Frame, area: Rect, app: &App, hitmap: &mut HitMap) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let labels = [
        ("PREV", Hit::Previous),
        (
            if app.is_playing() { "PAUSE" } else { "PLAY" },
            Hit::PlayPause,
        ),
        ("NEXT", Hit::Next),
    ];
    let mut x = area.x;
    for (label, hit) in labels {
        let width = label.chars().count() as u16 + 2;
        if x.saturating_add(width) > area.x.saturating_add(area.width) {
            break;
        }
        let rect = Rect::new(x, area.y, width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(ACCENT)
                    .bg(SELECTION)
                    .add_modifier(Modifier::BOLD),
            ))),
            rect,
        );
        hitmap.push(rect, hit);
        x = x.saturating_add(width + 1);
    }
    if area.height > 1 {
        let shuffle = app
            .status
            .as_ref()
            .map(|status| status.playback.shuffle)
            .unwrap_or(false);
        let repeat = app
            .status
            .as_ref()
            .map(|status| status.playback.repeat)
            .unwrap_or_default();
        let shuffle_style = if shuffle {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG_MUTED)
        };
        let repeat_style = if matches!(repeat, ytermusicd_api::RepeatMode::Off) {
            Style::default().fg(FG_MUTED)
        } else {
            Style::default().fg(ACCENT_2).add_modifier(Modifier::BOLD)
        };
        let shuffle_rect = Rect::new(area.x, area.y.saturating_add(1), 7, 1);
        let repeat_rect = Rect::new(area.x.saturating_add(8), area.y.saturating_add(1), 10, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" SHUF ", shuffle_style))),
            shuffle_rect,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" REP:{repeat:?} "),
                repeat_style,
            ))),
            repeat_rect,
        );
        hitmap.push(shuffle_rect, Hit::Shuffle);
        hitmap.push(repeat_rect, Hit::Repeat);
    }
    if area.height > 2 {
        let volume = app
            .status
            .as_ref()
            .map(|status| status.playback.volume)
            .unwrap_or(0);
        let label = Rect::new(area.x, area.y.saturating_add(2), 5, 1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " VOL ",
                Style::default().fg(FG_MUTED),
            ))),
            label,
        );
        let gauge_area = Rect::new(
            area.x.saturating_add(4),
            area.y.saturating_add(2),
            area.width.saturating_sub(9),
            1,
        );
        let gauge = LineGauge::default()
            .ratio(f64::from(volume) / 100.0)
            .label(Line::from(format!("{volume:>3}%")))
            .filled_style(Style::default().fg(ACCENT_2))
            .unfilled_style(Style::default().fg(BG));
        frame.render_widget(gauge, gauge_area);
        hitmap.push(gauge_area, Hit::Volume);
    }
    if area.height > 3 {
        let queue = app.queue.items.len();
        let (_, _, _, _, failed) = download_counts(app);
        let text = if failed > 0 {
            format!("queue {queue} · {failed} failed")
        } else {
            format!("queue {queue}")
        };
        let style = if failed > 0 {
            Style::default().fg(WARNING)
        } else {
            Style::default().fg(FG_MUTED)
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))),
            Rect::new(area.x, area.y.saturating_add(3), area.width, 1),
        );
    }
}
