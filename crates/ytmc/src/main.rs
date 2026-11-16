//! Terminal client for the `ytermusicd` daemon.

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
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::time::MissedTickBehavior;
use ytermusicd_client::DaemonClient;

use crate::app::App;
use crate::event::{Command, DaemonEvent};

/// Command-line options.
#[derive(Debug, Parser)]
#[command(
    name = "ytmc",
    version,
    about = "Terminal client for the ytermusicd daemon"
)]
struct Cli {
    /// Daemon URL.
    #[arg(long, env = "YTERMUSICD_URL", default_value = ytermusicd_client::DEFAULT_URL)]
    url: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli).await {
        restore_terminal();
        eprintln!("ytmc: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> io::Result<()> {
    let client = DaemonClient::new(cli.url.clone()).map_err(io::Error::other)?;
    let (event_tx, mut event_rx) = unbounded_channel::<DaemonEvent>();
    let (command_tx, command_rx) = unbounded_channel::<Command>();
    tokio::spawn(daemon::run_events(client.clone(), event_tx.clone()));
    tokio::spawn(daemon::run_commands(client, command_rx, event_tx));

    let mut app = App::new(cli.url);
    for command in app.initial_commands() {
        let _ = command_tx.send(command);
    }

    let mut terminal = setup_terminal()?;
    let _guard = TerminalGuard;
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
        let cli = Cli::try_parse_from(["ytmc"]).unwrap();
        assert_eq!(cli.url, ytermusicd_client::DEFAULT_URL);
    }

    #[test]
    fn parses_a_url() {
        let cli = Cli::try_parse_from(["ytmc", "--url", "http://localhost:9000"]).unwrap();
        assert_eq!(cli.url, "http://localhost:9000");
    }
}
