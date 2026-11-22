//! Terminal client for the `recvrd` daemon.

mod app;
mod cover;
mod daemon;
mod event;
mod mouse;
mod ui;

use std::io;
use std::panic;
use std::time::Duration;

use clap::Parser;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui_image::picker::{Picker, ProtocolType};
use recvrd_client::DaemonClient;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::time::MissedTickBehavior;

use crate::app::App;
use crate::event::{Command, DaemonEvent};

/// Old environment names and their new names.
const LEGACY_ENV: [(&str, &str); 3] = [
    ("RECVRD_URL", "YTERMUSICD_URL"),
    ("RECVC_COVER_MODE", "YTMC_COVER_MODE"),
    ("RECVC_DEBUG", "YTMC_DEBUG"),
];

/// Return the new environment values that come from an old name.
fn legacy_env_updates<F>(mut get: F) -> Vec<(&'static str, std::ffi::OsString)>
where
    F: FnMut(&str) -> Option<std::ffi::OsString>,
{
    let mut updates = Vec::new();
    for (new, old) in LEGACY_ENV {
        if get(new).is_none()
            && let Some(value) = get(old)
        {
            updates.push((new, value));
        }
    }
    updates
}

/// Copy old environment names to the new names when the new name is absent.
fn apply_legacy_env() {
    for (name, value) in legacy_env_updates(|name| std::env::var_os(name)) {
        // SAFETY: the process is single-threaded at this point.
        unsafe { std::env::set_var(name, value) };
    }
}

/// Command-line options.
#[derive(Debug, Parser)]
#[command(
    name = "recvc",
    version,
    about = "Terminal client for the recvrd daemon"
)]
struct Cli {
    /// Daemon URL.
    #[arg(long, env = "RECVRD_URL", default_value = recvrd_client::DEFAULT_URL)]
    url: String,
    /// Cover art rendering: `auto` uses a terminal graphics protocol
    /// when one is detected, `halfblocks` always uses half-block cells.
    #[arg(long, env = "RECVC_COVER_MODE", default_value = "auto")]
    cover_mode: CoverMode,
    /// Include the endpoint and the raw daemon response (truncated)
    /// in invalid-response errors, instead of only the serde error.
    /// Useful when the daemon and client disagree about a response
    /// shape. Also settable with `RECVC_DEBUG=true`.
    #[arg(long, env = "RECVC_DEBUG")]
    debug: bool,
}

/// How cover art is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum CoverMode {
    /// Use a terminal graphics protocol when detected, half-blocks
    /// otherwise.
    Auto,
    /// Always use the half-block renderer.
    Halfblocks,
}

fn main() {
    apply_legacy_env();
    run_main();
}

#[tokio::main]
async fn run_main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli).await {
        restore_terminal();
        eprintln!("recvc: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> io::Result<()> {
    let mut client = DaemonClient::new(cli.url.clone()).map_err(io::Error::other)?;
    client.set_debug(cli.debug);
    let (event_tx, mut event_rx) = unbounded_channel::<DaemonEvent>();
    let (command_tx, command_rx) = unbounded_channel::<Command>();
    tokio::spawn(daemon::run_events(client.clone(), event_tx.clone()));
    tokio::spawn(daemon::run_commands(client, command_rx, event_tx));

    let mut terminal = setup_terminal()?;
    let _guard = TerminalGuard;
    // Detect graphics support after entering the alternate screen and
    // before reading terminal events: the query writes escape
    // sequences to stdout and reads the reply from stdin.
    let mut app = App::new(cli.url);
    app.set_cover_picker(cover_picker(cli.cover_mode));
    for command in app.initial_commands() {
        let _ = command_tx.send(command);
    }

    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| ui::render(frame, &mut app))?;
        if app.quit {
            break;
        }
        tokio::select! {
            next = input.next() => {
                match next {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        dispatch(&command_tx, app.handle_key(key));
                    }
                    Some(Ok(Event::Mouse(mouse))) => {
                        dispatch(&command_tx, app.handle_mouse(mouse));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        return Err(error);
                    }
                    None => break,
                }
            }
            Some(event) = event_rx.recv() => {
                dispatch(&command_tx, app.handle_daemon_event(event));
            }
            _ = tick.tick() => {
                dispatch(&command_tx, app.tick());
            }
        }
    }
    Ok(())
}

fn cover_picker(mode: CoverMode) -> Option<Picker> {
    if mode == CoverMode::Halfblocks {
        return None;
    }
    match Picker::from_query_stdio() {
        Ok(picker) if picker.protocol_type() != ProtocolType::Halfblocks => Some(picker),
        _ => None,
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide) {
        let _ = disable_raw_mode();
        return Err(error);
    }
    let backend = CrosstermBackend::new(stdout);
    match Terminal::new(backend) {
        Ok(terminal) => {
            let old_hook = panic::take_hook();
            panic::set_hook(Box::new(move |info| {
                restore_terminal();
                old_hook(info);
            }));
            Ok(terminal)
        }
        Err(error) => {
            restore_terminal();
            Err(error)
        }
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        Show
    );
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn dispatch(sender: &UnboundedSender<Command>, commands: Vec<Command>) {
    for command in commands {
        let _ = sender.send(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default_url() {
        let cli = Cli::try_parse_from(["recvc"]).unwrap();
        assert_eq!(cli.url, recvrd_client::DEFAULT_URL);
    }

    #[test]
    fn parses_a_url() {
        let cli = Cli::try_parse_from(["recvc", "--url", "http://localhost:9000"]).unwrap();
        assert_eq!(cli.url, "http://localhost:9000");
    }

    #[test]
    fn parses_cover_mode() {
        let cli = Cli::try_parse_from(["recvc", "--cover-mode", "halfblocks"]).unwrap();
        assert_eq!(cli.cover_mode, CoverMode::Halfblocks);
        let cli = Cli::try_parse_from(["recvc"]).unwrap();
        assert_eq!(cli.cover_mode, CoverMode::Auto);
    }

    #[test]
    fn parses_the_debug_flag() {
        let cli = Cli::try_parse_from(["recvc"]).unwrap();
        assert!(!cli.debug);
        let cli = Cli::try_parse_from(["recvc", "--debug"]).unwrap();
        assert!(cli.debug);
    }

    #[test]
    fn legacy_environment_fills_a_missing_new_name() {
        let updates = legacy_env_updates(|name| match name {
            "YTERMUSICD_URL" => Some("http://old".into()),
            _ => None,
        });
        assert_eq!(updates, vec![("RECVRD_URL", "http://old".into())]);
    }

    #[test]
    fn legacy_environment_does_not_replace_a_new_name() {
        let updates = legacy_env_updates(|name| match name {
            "RECVRD_URL" => Some("http://new".into()),
            "YTERMUSICD_URL" => Some("http://old".into()),
            _ => None,
        });
        assert!(
            updates.iter().all(|(name, _)| *name != "RECVRD_URL"),
            "{updates:?}"
        );
    }
}
