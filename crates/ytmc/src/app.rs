//! Application state and input handling for the terminal UI.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::widgets::{ListState, TableState};
use ytermusicd_api::{
    AuthState, DownloadStatus, PlaybackState, Playlist, PlaylistDetail, Queue, RepeatMode,
    SearchResults, Status, Track,
};

use crate::cover::CoverArt;
use crate::event::{Command, DaemonEvent, ToastLevel};
use crate::mouse::{Hit, HitMap, Nav};

/// Time window for a two-key sequence such as `gg` or `dd`.
pub const KEY_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(650);
/// Time window for a double click.
pub const DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(400);
/// Time before a toast expires.
pub const TOAST_LIFETIME: Duration = Duration::from_secs(5);
/// Time before the search input sends a query.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// The main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The playlist library.
    Library,
    /// One playlist.
    Playlist,
    /// The playback queue.
    Queue,
}

/// The keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The main view.
    Main,
    /// The sidebar playlist list.
    Sidebar,
}

/// The daemon connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// The client is connecting.
    Connecting,
    /// The event stream is open.
    Online,
    /// The event stream stopped. A reconnect follows.
    Offline,
}

/// An overlay above the main view.
#[derive(Debug)]
pub enum Overlay {
    /// The key and mouse help.
    Help,
    /// The search palette.
    Search,
    /// A context menu.
    Context(ContextMenu),
    /// A text input dialog.
    Input(InputModal),
    /// A confirmation dialog.
    Confirm(ConfirmModal),
    /// A playlist picker.
    PickPlaylist(PickPlaylistModal),
}

/// A context menu.
#[derive(Debug)]
pub struct ContextMenu {
    /// Left position in terminal cells.
    pub x: u16,
    /// Top position in terminal cells.
    pub y: u16,
    /// Menu title.
    pub title: String,
    /// Menu items.
    pub items: Vec<ContextItem>,
    /// Selected item.
    pub selected: usize,
}

/// One context menu item.
#[derive(Debug, Clone)]
pub struct ContextItem {
    /// Text to show.
    pub label: String,
    /// Action to run.
    pub action: ContextAction,
}

/// An action in a context menu.
#[derive(Debug, Clone)]
pub enum ContextAction {
    /// Play a track now.
    PlayTrack(Track),
    /// Add a track to the queue.
    QueueTrack(Track),
    /// Add a track to a playlist.
    AddTrackToPlaylist(Track),
    /// Open a playlist.
    OpenPlaylist(String),
    /// Play a playlist.
    PlayPlaylist(String),
    /// Rename a playlist.
    RenamePlaylist(String),
    /// Delete a playlist.
    DeletePlaylist(String),
    /// Play a queue index.
    PlayQueueIndex(usize),
    /// Remove a queue index.
    RemoveQueue(usize),
    /// Move a queue item up.
    MoveQueueUp(usize),
    /// Move a queue item to play next.
    PlayQueueNext(usize),
    /// Move a queue item down.
    MoveQueueDown(usize),
    /// Clear the queue.
    ClearQueue,
    /// Retry a failed download.
    RetryDownload(String),
}

/// A text input action.
#[derive(Debug, Clone)]
pub enum InputAction {
    /// Create a playlist.
    CreatePlaylist,
    /// Rename a playlist.
    RenamePlaylist(String),
}

/// A text input dialog.
#[derive(Debug)]
pub struct InputModal {
    /// Dialog title.
    pub title: String,
    /// Current text.
    pub value: String,
    /// Action to run on submit.
    pub action: InputAction,
}

/// A confirmation action.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Delete a playlist.
    DeletePlaylist(String),
    /// Remove one playlist track.
    RemovePlaylistTrack { id: String, index: usize },
    /// Remove one queue item.
    RemoveQueue(usize),
    /// Clear the queue.
    ClearQueue,
}

/// A confirmation dialog.
#[derive(Debug)]
pub struct ConfirmModal {
    /// Dialog title.
    pub title: String,
    /// Message to show.
    pub message: String,
    /// Action to run on confirm.
    pub action: ConfirmAction,
}

/// A playlist picker.
#[derive(Debug)]
pub struct PickPlaylistModal {
    /// Track to add.
    pub track: Track,
    /// Selected playlist.
    pub selected: usize,
}

/// The search palette state.
#[derive(Debug)]
pub struct SearchState {
    /// Current input text.
    pub input: String,
    /// The query that produced the current results.
    pub query: String,
    /// Current results.
    pub results: SearchResults,
    /// Selected result.
    pub selected: usize,
    /// True while a search request runs.
    pub loading: bool,
    /// Time to send the next query.
    pub send_at: Option<Instant>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            input: String::new(),
            query: String::new(),
            results: SearchResults {
                tracks: Vec::new(),
                playlists: Vec::new(),
            },
            selected: 0,
            loading: false,
            send_at: None,
        }
    }
}

impl SearchState {
    /// Return the combined result list.
    pub fn entries(&self) -> Vec<SearchEntry> {
        let mut entries =
            Vec::with_capacity(self.results.tracks.len() + self.results.playlists.len());
        entries.extend(self.results.tracks.iter().cloned().map(SearchEntry::Track));
        entries.extend(
            self.results
                .playlists
                .iter()
                .cloned()
                .map(SearchEntry::Playlist),
        );
        entries
    }

    /// Return the number of results.
    pub fn len(&self) -> usize {
        self.results.tracks.len() + self.results.playlists.len()
    }

    /// Return true when there are no results.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the selected result.
    pub fn selected_entry(&self) -> Option<SearchEntry> {
        self.entries().get(self.selected).cloned()
    }
}

/// One search result.
#[derive(Debug, Clone)]
pub enum SearchEntry {
    /// A track.
    Track(Track),
    /// A playlist.
    Playlist(Playlist),
}

/// A user-visible message.
#[derive(Debug)]
pub struct Toast {
    /// Message severity.
    pub level: ToastLevel,
    /// Message text.
    pub message: String,
    /// Creation time.
    pub created: Instant,
}

/// The terminal UI state.
pub struct App {
    /// Daemon URL.
    pub url: String,
    /// Connection state.
    pub connection: Connection,
    /// Connection error text.
    pub connection_message: Option<String>,
    /// Latest status.
    pub status: Option<Status>,
    /// Latest queue.
    pub queue: Queue,
    /// Library playlists.
    pub playlists: Vec<Playlist>,
    /// Open playlist.
    pub playlist: Option<PlaylistDetail>,
    /// Playlist id that is loading.
    pub playlist_request: Option<String>,
    /// Current screen.
    pub screen: Screen,
    /// Current focus.
    pub focus: Focus,
    /// Library selection.
    pub library_selection: usize,
    /// Playlist selection.
    pub playlist_selection: usize,
    /// Queue selection.
    pub queue_selection: usize,
    /// Sidebar selection.
    pub sidebar_selection: usize,
    /// Library table state.
    pub library_state: TableState,
    /// Playlist table state.
    pub playlist_state: TableState,
    /// Queue list state.
    pub queue_state: ListState,
    /// Sidebar list state.
    pub sidebar_state: ListState,
    /// Search palette state.
    pub search: SearchState,
    /// Current overlay.
    pub overlay: Option<Overlay>,
    /// Active toasts.
    pub toasts: Vec<Toast>,
    /// Cover art for the current track.
    pub cover: Option<CoverArt>,
    /// Video id for the cover request.
    pub cover_for: Option<String>,
    /// Pending `g` or `d` key.
    pub pending_key: Option<(char, Instant)>,
    /// Last click.
    pub last_click: Option<(Instant, u16, u16, Hit)>,
    /// Hit regions for the current frame.
    pub hitmap: HitMap,
    /// True when the UI must exit.
    pub quit: bool,
}

impl App {
    /// Create an app for the daemon URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            connection: Connection::Connecting,
            connection_message: None,
            status: None,
            queue: Queue::default(),
            playlists: Vec::new(),
            playlist: None,
            playlist_request: None,
            screen: Screen::Library,
            focus: Focus::Main,
            library_selection: 0,
            playlist_selection: 0,
            queue_selection: 0,
            sidebar_selection: 0,
            library_state: TableState::default(),
            playlist_state: TableState::default(),
            queue_state: ListState::default(),
            sidebar_state: ListState::default(),
            search: SearchState::default(),
            overlay: None,
            toasts: Vec::new(),
            cover: None,
            cover_for: None,
            pending_key: None,
            last_click: None,
            hitmap: HitMap::default(),
            quit: false,
        }
    }

    /// Return the commands to run at startup.
    pub fn initial_commands(&self) -> Vec<Command> {
        vec![Command::RefreshPlaylists]
    }

    /// Return the current track.
    pub fn current_track(&self) -> Option<Track> {
        self.status
            .as_ref()
            .and_then(|status| status.playback.current.clone())
            .or_else(|| {
                self.queue
                    .current
                    .and_then(|index| self.queue.items.get(index))
                    .map(|item| item.track.clone())
            })
    }

    /// Return the current video id.
    pub fn current_video_id(&self) -> Option<String> {
        self.current_track().map(|track| track.video_id)
    }

    /// Return the current playback position in seconds.
    pub fn position_secs(&self) -> f64 {
        self.status
            .as_ref()
            .map(|status| status.playback.position_secs)
            .unwrap_or(0.0)
    }

    /// Return the duration of the current track in seconds.
    pub fn duration_secs(&self) -> Option<f64> {
        self.current_track()
            .and_then(|track| parse_duration(&track.duration))
    }

    /// Return true when playback is active.
    pub fn is_playing(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.playback.state == PlaybackState::Playing)
    }

    /// Return true when the daemon reports an auth problem.
    pub fn auth_problem(&self) -> Option<String> {
        let auth = &self.status.as_ref()?.auth;
        if auth.state == AuthState::Ok {
            return None;
        }
        Some(
            auth.message
                .clone()
                .unwrap_or_else(|| "Auth is not valid.".to_string()),
        )
    }

    /// Return the selected playlist.
    pub fn selected_playlist(&self) -> Option<&Playlist> {
        match self.focus {
            Focus::Sidebar => self.playlists.get(self.sidebar_selection),
            Focus::Main if self.screen == Screen::Library => {
                self.playlists.get(self.library_selection)
            }
            _ => None,
        }
    }

    /// Return the selected track.
    pub fn selected_track(&self) -> Option<&Track> {
        match self.screen {
            Screen::Playlist => self
                .playlist
                .as_ref()
                .and_then(|playlist| playlist.tracks.get(self.playlist_selection)),
            Screen::Queue => self
                .queue
                .items
                .get(self.queue_selection)
                .map(|item| &item.track),
            Screen::Library => None,
        }
    }

    /// Apply a daemon event and return commands to run.
    pub fn handle_daemon_event(&mut self, event: DaemonEvent) -> Vec<Command> {
        match event {
            DaemonEvent::Connected => {
                self.connection = Connection::Online;
                self.connection_message = None;
                let mut commands = vec![Command::RefreshPlaylists];
                if self.cover.is_none() {
                    self.cover_for = None;
                    commands.extend(self.ensure_cover());
                }
                commands
            }
            DaemonEvent::Disconnected(error) => {
                self.connection = Connection::Offline;
                self.connection_message = Some(error.message);
                Vec::new()
            }
            DaemonEvent::Snapshot { status, queue } => {
                self.connection = Connection::Online;
                self.connection_message = None;
                let mut commands = self.apply_status(status);
                commands.extend(self.apply_queue(queue));
                commands
            }
            DaemonEvent::Playback(playback) => {
                let previous = self.current_video_id();
                if let Some(status) = &mut self.status {
                    status.playback = playback;
                }
                if previous != self.current_video_id() {
                    self.cover = None;
                    self.cover_for = None;
                }
                self.ensure_cover()
            }
            DaemonEvent::Queue(queue) => self.apply_queue(queue),
            DaemonEvent::Download { video_id, status } => {
                for item in &mut self.queue.items {
                    if item.track.video_id == video_id {
                        item.download = status.clone();
                        break;
                    }
                }
                if let DownloadStatus::Failed { message } = status {
                    let message = message.unwrap_or_else(|| "Download failed.".to_string());
                    self.show_toast(ToastLevel::Error, message);
                }
                Vec::new()
            }
            DaemonEvent::TrackEnded => Vec::new(),
            DaemonEvent::Error(error) => {
                self.show_toast(ToastLevel::Error, error.message);
                Vec::new()
            }
            DaemonEvent::LibraryChanged => self.refresh_library(),
            DaemonEvent::Status(status) => self.apply_status(status),
            DaemonEvent::Playlists(playlists) => {
                self.apply_playlists(playlists);
                Vec::new()
            }
            DaemonEvent::Playlist(detail) => {
                let fresh = self.playlist_request.is_some();
                let requested = self.playlist_request.as_deref();
                if requested == Some(detail.playlist.id.as_str())
                    || self.open_playlist_id().as_deref() == Some(detail.playlist.id.as_str())
                {
                    self.playlist_request = None;
                    let tracks = detail.tracks.len();
                    self.playlist = Some(detail);
                    if fresh {
                        self.playlist_selection = 0;
                    } else {
                        self.playlist_selection = clamp_index(self.playlist_selection, tracks);
                    }
                    self.sync_selection();
                }
                Vec::new()
            }
            DaemonEvent::PlaylistError { id, message } => {
                if self.playlist_request.as_deref() == Some(id.as_str()) {
                    self.playlist_request = None;
                    self.playlist = None;
                    self.go_library();
                }
                self.show_toast(ToastLevel::Error, message);
                Vec::new()
            }
            DaemonEvent::PlaylistTracks { id, tracks } => {
                let matches = self
                    .playlist
                    .as_ref()
                    .is_some_and(|playlist| playlist.playlist.id == id);
                if matches {
                    if let Some(playlist) = &mut self.playlist {
                        playlist.tracks = tracks;
                    }
                    let len = self
                        .playlist
                        .as_ref()
                        .map(|playlist| playlist.tracks.len())
                        .unwrap_or(0);
                    self.playlist_selection = clamp_index(self.playlist_selection, len);
                    self.sync_selection();
                }
                Vec::new()
            }
            DaemonEvent::Search { query, results } => {
                if query == self.search.query {
                    self.search.results = results;
                    self.search.loading = false;
                    self.search.selected = clamp_index(self.search.selected, self.search.len());
                }
                Vec::new()
            }
            DaemonEvent::SearchError { query, message } => {
                if query == self.search.query {
                    self.search.loading = false;
                    self.show_toast(ToastLevel::Error, message);
                }
                Vec::new()
            }
            DaemonEvent::Cover { video_id, art } => {
                if self.cover_for.as_deref() == Some(video_id.as_str()) {
                    self.cover = art;
                    if self.cover.is_none() {
                        self.cover_for = None;
                    }
                }
                Vec::new()
            }
            DaemonEvent::Toast { level, message } => {
                self.show_toast(level, message);
                Vec::new()
            }
        }
    }

    fn apply_status(&mut self, status: Status) -> Vec<Command> {
        let previous = self.current_video_id();
        self.status = Some(status);
        if previous != self.current_video_id() {
            self.cover = None;
            self.cover_for = None;
        }
        self.sync_selection();
        self.ensure_cover()
    }

    fn apply_queue(&mut self, queue: Queue) -> Vec<Command> {
        let previous = self.current_video_id();
        let len = queue.items.len();
        self.queue = queue;
        self.queue_selection = clamp_index(self.queue_selection, len);
        if let Some(status) = &mut self.status {
            status.queue_length = len;
        }
        if previous != self.current_video_id() {
            self.cover = None;
            self.cover_for = None;
        }
        self.sync_selection();
        self.ensure_cover()
    }

    fn apply_playlists(&mut self, playlists: Vec<Playlist>) {
        self.playlists = playlists;
        self.library_selection = clamp_index(self.library_selection, self.playlists.len());
        self.sidebar_selection = clamp_index(self.sidebar_selection, self.playlists.len());
        let open_id = self.open_playlist_id();
        if let Some(id) = open_id {
            match self
                .playlists
                .iter()
                .find(|playlist| playlist.id == id)
                .cloned()
            {
                Some(updated) => {
                    if let Some(detail) = &mut self.playlist {
                        detail.playlist = updated;
                    }
                }
                None if self.screen == Screen::Playlist => self.go_library(),
                None => {}
            }
        }
        self.sync_selection();
    }

    fn ensure_cover(&mut self) -> Vec<Command> {
        let Some(track) = self.current_track() else {
            return Vec::new();
        };
        if self.cover_for.as_deref() == Some(track.video_id.as_str()) {
            return Vec::new();
        }
        self.cover_for = Some(track.video_id.clone());
        vec![Command::LoadCover(track.video_id)]
    }

    fn refresh_library(&self) -> Vec<Command> {
        let mut commands = vec![Command::RefreshPlaylists];
        if let Some(id) = self.open_playlist_id() {
            commands.push(Command::RefreshPlaylist(id));
        }
        commands
    }

    fn open_playlist_id(&self) -> Option<String> {
        self.playlist
            .as_ref()
            .map(|playlist| playlist.playlist.id.clone())
    }

    /// Sync the widget states with the selection values.
    pub fn sync_selection(&mut self) {
        self.library_state.select(if self.playlists.is_empty() {
            None
        } else {
            Some(self.library_selection)
        });
        let tracks = self
            .playlist
            .as_ref()
            .map(|playlist| playlist.tracks.len())
            .unwrap_or(0);
        self.playlist_state.select(if tracks == 0 {
            None
        } else {
            Some(self.playlist_selection)
        });
        self.queue_state.select(if self.queue.items.is_empty() {
            None
        } else {
            Some(self.queue_selection)
        });
        self.sidebar_state.select(if self.playlists.is_empty() {
            None
        } else {
            Some(self.sidebar_selection)
        });
    }

    /// Apply the time-based updates.
    pub fn tick(&mut self) -> Vec<Command> {
        let now = Instant::now();
        if let Some((_, at)) = self.pending_key
            && now.duration_since(at) >= KEY_SEQUENCE_TIMEOUT
        {
            self.pending_key = None;
        }
        self.toasts
            .retain(|toast| now.duration_since(toast.created) < TOAST_LIFETIME);
        if let Some(at) = self.search.send_at
            && now >= at
        {
            self.search.send_at = None;
            if self.search.input.trim().is_empty() {
                self.search.query.clear();
                self.search.results = SearchResults {
                    tracks: Vec::new(),
                    playlists: Vec::new(),
                };
                self.search.loading = false;
            } else if self.search.input != self.search.query {
                return self.submit_search();
            }
        }
        Vec::new()
    }

    /// Handle a key event and return commands to run.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if key.kind == crossterm::event::KeyEventKind::Release {
            return Vec::new();
        }
        if is_ctrl(key, 'c') {
            self.quit = true;
            return Vec::new();
        }
        if self.overlay.is_some() {
            return self.handle_overlay_key(key);
        }
        if is_ctrl(key, 'k') {
            self.open_search();
            return Vec::new();
        }

        let now = Instant::now();
        if let Some((pending, at)) = self.pending_key.take()
            && now.duration_since(at) < KEY_SEQUENCE_TIMEOUT
        {
            if pending == 'g' && is_char(key, 'g') {
                self.jump_first();
                return Vec::new();
            }
            if pending == 'd' && is_char(key, 'd') {
                return self.delete_selected();
            }
        }

        if is_char(key, '?') {
            self.overlay = Some(Overlay::Help);
            return Vec::new();
        }
        if is_char(key, '/') {
            self.open_search();
            return Vec::new();
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.half_page(1),
                KeyCode::Char('u') => self.half_page(-1),
                KeyCode::Char('r') => return self.refresh_library(),
                KeyCode::Left => return self.previous_track(),
                KeyCode::Right => return self.next_track(),
                _ => {}
            }
            return Vec::new();
        }

        self.handle_normal_key(key, now)
    }

    fn handle_normal_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Command> {
        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Main => Focus::Sidebar,
                    Focus::Sidebar => Focus::Main,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Home => self.jump_first(),
            KeyCode::End | KeyCode::Char('G') => self.jump_last(),
            KeyCode::Char('g') => self.pending_key = Some(('g', now)),
            KeyCode::Char('d') => self.pending_key = Some(('d', now)),
            KeyCode::Char('J') => return self.move_item(1),
            KeyCode::Char('K') => return self.move_item(-1),
            KeyCode::Char('D') => return self.clear_queue_confirmation(),
            KeyCode::Char('n') => self.open_create_playlist(),
            KeyCode::Char('r') => return self.open_rename_playlist(),
            KeyCode::Char('p') => return self.play_selected_playlist(),
            KeyCode::Char('a') => return self.add_selected_to_playlist(),
            KeyCode::Char('x') => return self.queue_selected(),
            KeyCode::Char('t') => return self.retry_selected(),
            KeyCode::Char('s') => return self.toggle_shuffle(),
            KeyCode::Char('R') => return self.cycle_repeat(),
            KeyCode::Char('+') | KeyCode::Char('=') => return self.adjust_volume(5),
            KeyCode::Char('-') | KeyCode::Char('_') => return self.adjust_volume(-5),
            KeyCode::Enter => return self.activate(),
            KeyCode::Char(' ') => return self.toggle_playback(),
            KeyCode::Left => return self.seek_relative(-5.0),
            KeyCode::Right => return self.seek_relative(5.0),
            KeyCode::Char('1') => self.go_library(),
            KeyCode::Char('2') => self.go_queue(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => {
                if self.screen != Screen::Library {
                    self.go_library();
                } else if self.focus == Focus::Sidebar {
                    self.focus = Focus::Main;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match &self.overlay {
            Some(Overlay::Help) => {
                if is_char(key, '?') || is_char(key, 'q') || key.code == KeyCode::Esc {
                    self.overlay = None;
                }
                Vec::new()
            }
            Some(Overlay::Search) => self.handle_search_key(key),
            Some(Overlay::Context(_)) => self.handle_context_key(key),
            Some(Overlay::Input(_)) => self.handle_input_key(key),
            Some(Overlay::Confirm(_)) => self.handle_confirm_key(key),
            Some(Overlay::PickPlaylist(_)) => self.handle_picker_key(key),
            None => Vec::new(),
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if key.code == KeyCode::Esc {
            self.close_overlay();
            return Vec::new();
        }
        if is_ctrl(key, 'u') {
            self.search.input.clear();
            self.schedule_search();
            return Vec::new();
        }
        if is_ctrl(key, 'x') {
            return self.queue_search_selected();
        }
        if is_ctrl(key, 'a') {
            return self.add_search_selected_to_playlist();
        }
        match key.code {
            KeyCode::Enter => {
                if self.search.input != self.search.query || self.search.is_empty() {
                    return self.submit_search();
                }
                self.activate_search_selected()
            }
            KeyCode::Backspace => {
                self.search.input.pop();
                self.schedule_search();
                Vec::new()
            }
            KeyCode::Up => {
                self.search.selected =
                    clamp_index(self.search.selected.saturating_sub(1), self.search.len());
                Vec::new()
            }
            KeyCode::Down => {
                self.search.selected = clamp_index(self.search.selected + 1, self.search.len());
                Vec::new()
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.search.input.push(character);
                self.schedule_search();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_context_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                Vec::new()
            }
            KeyCode::Up => {
                if let Some(Overlay::Context(context)) = &mut self.overlay {
                    context.selected = context.selected.saturating_sub(1);
                }
                Vec::new()
            }
            KeyCode::Down => {
                if let Some(Overlay::Context(context)) = &mut self.overlay {
                    context.selected =
                        (context.selected + 1).min(context.items.len().saturating_sub(1));
                }
                Vec::new()
            }
            KeyCode::Enter => {
                let action = match &self.overlay {
                    Some(Overlay::Context(context)) => context
                        .items
                        .get(context.selected)
                        .map(|item| item.action.clone()),
                    _ => None,
                };
                self.overlay = None;
                action
                    .map(|action| self.run_context_action(action))
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                Vec::new()
            }
            KeyCode::Enter => {
                let (value, action) = match &self.overlay {
                    Some(Overlay::Input(input)) => {
                        (input.value.trim().to_string(), input.action.clone())
                    }
                    _ => return Vec::new(),
                };
                if value.is_empty() {
                    return Vec::new();
                }
                self.overlay = None;
                match action {
                    InputAction::CreatePlaylist => vec![Command::CreatePlaylist { title: value }],
                    InputAction::RenamePlaylist(id) => {
                        vec![Command::RenamePlaylist { id, title: value }]
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(Overlay::Input(input)) = &mut self.overlay {
                    input.value.pop();
                }
                Vec::new()
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(Overlay::Input(input)) = &mut self.overlay {
                    input.value.clear();
                }
                Vec::new()
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(Overlay::Input(input)) = &mut self.overlay {
                    input.value.push(character);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.overlay = None;
                Vec::new()
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let action = match &self.overlay {
                    Some(Overlay::Confirm(confirm)) => Some(confirm.action.clone()),
                    _ => None,
                };
                self.overlay = None;
                action
                    .map(|action| self.run_confirm_action(action))
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                Vec::new()
            }
            KeyCode::Up => {
                if let Some(Overlay::PickPlaylist(picker)) = &mut self.overlay {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Vec::new()
            }
            KeyCode::Down => {
                if let Some(Overlay::PickPlaylist(picker)) = &mut self.overlay {
                    picker.selected =
                        (picker.selected + 1).min(self.playlists.len().saturating_sub(1));
                }
                Vec::new()
            }
            KeyCode::Enter => {
                let (track, selected) = match &self.overlay {
                    Some(Overlay::PickPlaylist(picker)) => (picker.track.clone(), picker.selected),
                    _ => return Vec::new(),
                };
                self.overlay = None;
                match self.playlists.get(selected).cloned() {
                    Some(playlist) => {
                        self.show_toast(ToastLevel::Info, format!("Added to {}.", playlist.name));
                        vec![Command::AddPlaylistTracks {
                            id: playlist.id,
                            video_ids: vec![track.video_id],
                        }]
                    }
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn open_search(&mut self) {
        self.search = SearchState::default();
        self.overlay = Some(Overlay::Search);
    }

    fn close_overlay(&mut self) {
        self.overlay = None;
        self.search = SearchState::default();
    }

    fn schedule_search(&mut self) {
        self.search.send_at = Some(Instant::now() + SEARCH_DEBOUNCE);
    }

    fn submit_search(&mut self) -> Vec<Command> {
        let query = self.search.input.trim().to_string();
        if query.is_empty() {
            return Vec::new();
        }
        self.search.query = query.clone();
        self.search.loading = true;
        self.search.selected = 0;
        vec![Command::Search(query)]
    }

    fn activate_search_selected(&mut self) -> Vec<Command> {
        match self.search.selected_entry() {
            Some(SearchEntry::Track(track)) => {
                self.overlay = None;
                vec![Command::PlayTrack(track)]
            }
            Some(SearchEntry::Playlist(playlist)) => {
                self.overlay = None;
                self.open_playlist(&playlist.id)
            }
            None => Vec::new(),
        }
    }

    fn queue_search_selected(&mut self) -> Vec<Command> {
        match self.search.selected_entry() {
            Some(SearchEntry::Track(track)) => {
                self.show_toast(ToastLevel::Info, format!("Queued {}.", track.title));
                vec![Command::QueueTrack(track)]
            }
            _ => Vec::new(),
        }
    }

    fn add_search_selected_to_playlist(&mut self) -> Vec<Command> {
        match self.search.selected_entry() {
            Some(SearchEntry::Track(track)) => {
                self.overlay = Some(Overlay::PickPlaylist(PickPlaylistModal {
                    track,
                    selected: 0,
                }));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn run_context_action(&mut self, action: ContextAction) -> Vec<Command> {
        match action {
            ContextAction::PlayTrack(track) => vec![Command::PlayTrack(track)],
            ContextAction::QueueTrack(track) => {
                self.show_toast(ToastLevel::Info, format!("Queued {}.", track.title));
                vec![Command::QueueTrack(track)]
            }
            ContextAction::AddTrackToPlaylist(track) => {
                self.overlay = Some(Overlay::PickPlaylist(PickPlaylistModal {
                    track,
                    selected: 0,
                }));
                Vec::new()
            }
            ContextAction::OpenPlaylist(id) => self.open_playlist(&id),
            ContextAction::PlayPlaylist(id) => vec![Command::PlayPlaylist(id)],
            ContextAction::RenamePlaylist(id) => {
                let value = self
                    .playlists
                    .iter()
                    .find(|playlist| playlist.id == id)
                    .map(|playlist| playlist.name.clone())
                    .unwrap_or_default();
                self.overlay = Some(Overlay::Input(InputModal {
                    title: "Rename playlist".to_string(),
                    value,
                    action: InputAction::RenamePlaylist(id),
                }));
                Vec::new()
            }
            ContextAction::DeletePlaylist(id) => {
                self.overlay = Some(Overlay::Confirm(ConfirmModal {
                    title: "Delete playlist".to_string(),
                    message: "Delete this playlist? This cannot be undone.".to_string(),
                    action: ConfirmAction::DeletePlaylist(id),
                }));
                Vec::new()
            }
            ContextAction::PlayQueueIndex(index) => self
                .queue
                .items
                .get(index)
                .map(|item| vec![Command::PlayTrack(item.track.clone())])
                .unwrap_or_default(),
            ContextAction::RemoveQueue(index) => {
                self.overlay = Some(Overlay::Confirm(ConfirmModal {
                    title: "Remove track".to_string(),
                    message: "Remove this track from the queue?".to_string(),
                    action: ConfirmAction::RemoveQueue(index),
                }));
                Vec::new()
            }
            ContextAction::MoveQueueUp(index) => self.reorder_queue(index, index.saturating_sub(1)),
            ContextAction::MoveQueueDown(index) => self.reorder_queue(index, index + 1),
            ContextAction::PlayQueueNext(index) => vec![Command::QueuePlayNext(index)],
            ContextAction::ClearQueue => self.clear_queue_confirmation(),
            ContextAction::RetryDownload(video_id) => vec![Command::RetryDownload(video_id)],
        }
    }

    fn run_confirm_action(&mut self, action: ConfirmAction) -> Vec<Command> {
        match action {
            ConfirmAction::DeletePlaylist(id) => {
                self.go_library();
                vec![Command::DeletePlaylist(id)]
            }
            ConfirmAction::RemovePlaylistTrack { id, index } => {
                vec![Command::RemovePlaylistTrack { id, index }]
            }
            ConfirmAction::RemoveQueue(index) => vec![Command::QueueRemove(index)],
            ConfirmAction::ClearQueue => vec![Command::QueueClear],
        }
    }

    /// Handle a mouse event and return commands to run.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<Command> {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(mouse.column, mouse.row),
            MouseEventKind::Down(MouseButton::Right) => self.right_click(mouse.column, mouse.row),
            MouseEventKind::ScrollDown => self.scroll(mouse.column, mouse.row, 3),
            MouseEventKind::ScrollUp => self.scroll(mouse.column, mouse.row, -3),
            _ => Vec::new(),
        }
    }

    fn click(&mut self, column: u16, row: u16) -> Vec<Command> {
        let Some((hit, rect)) = self.hitmap.hit_with_rect(column, row) else {
            if self.overlay.is_some() {
                self.close_overlay();
            }
            return Vec::new();
        };
        if self.overlay.is_some() {
            match hit {
                Hit::Overlay
                | Hit::SearchRow(_)
                | Hit::ContextRow(_)
                | Hit::PlaylistPickerRow(_)
                | Hit::Toast(_) => {}
                _ => {
                    self.close_overlay();
                    return Vec::new();
                }
            }
        }
        match hit {
            Hit::Overlay => Vec::new(),
            Hit::Nav(nav) => {
                match nav {
                    Nav::Library => self.go_library(),
                    Nav::Queue => self.go_queue(),
                }
                Vec::new()
            }
            Hit::SidebarPlaylist(index) => {
                self.focus = Focus::Sidebar;
                self.sidebar_selection = clamp_index(index, self.playlists.len());
                self.sync_selection();
                let id = self
                    .playlists
                    .get(index)
                    .map(|playlist| playlist.id.clone());
                id.map(|id| self.open_playlist(&id)).unwrap_or_default()
            }
            Hit::LibraryRow(index) => {
                self.focus = Focus::Main;
                self.library_selection = clamp_index(index, self.playlists.len());
                self.sync_selection();
                if self.is_double_click(column, row, hit) {
                    self.activate()
                } else {
                    Vec::new()
                }
            }
            Hit::TrackRow(index) => {
                self.focus = Focus::Main;
                self.playlist_selection = clamp_index(
                    index,
                    self.playlist
                        .as_ref()
                        .map(|playlist| playlist.tracks.len())
                        .unwrap_or(0),
                );
                self.sync_selection();
                if self.is_double_click(column, row, hit) {
                    self.activate()
                } else {
                    Vec::new()
                }
            }
            Hit::QueueRow(index) => {
                self.focus = Focus::Main;
                self.queue_selection = clamp_index(index, self.queue.items.len());
                self.sync_selection();
                if self.is_double_click(column, row, hit) {
                    self.activate()
                } else {
                    Vec::new()
                }
            }
            Hit::SearchRow(index) => {
                self.search.selected = clamp_index(index, self.search.len());
                self.activate_search_selected()
            }
            Hit::ContextRow(index) => {
                let action = match &self.overlay {
                    Some(Overlay::Context(context)) => {
                        context.items.get(index).map(|item| item.action.clone())
                    }
                    _ => None,
                };
                self.overlay = None;
                action
                    .map(|action| self.run_context_action(action))
                    .unwrap_or_default()
            }
            Hit::PlaylistPickerRow(index) => {
                if let Some(Overlay::PickPlaylist(picker)) = &mut self.overlay {
                    picker.selected = index;
                }
                self.handle_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::PlayPause => self.toggle_playback(),
            Hit::Next => self.next_track(),
            Hit::Previous => self.previous_track(),
            Hit::Shuffle => self.toggle_shuffle(),
            Hit::Repeat => self.cycle_repeat(),
            Hit::Progress => {
                let fraction = fraction(column, rect);
                if let Some(duration) = self.duration_secs() {
                    return vec![Command::Seek(duration * fraction)];
                }
                Vec::new()
            }
            Hit::Volume => {
                let volume = (fraction(column, rect) * 100.0).round() as u8;
                vec![Command::Volume(volume.min(100))]
            }
            Hit::Toast(index) => {
                if index < self.toasts.len() {
                    self.toasts.remove(index);
                }
                Vec::new()
            }
            Hit::MainPane => {
                self.focus = Focus::Main;
                Vec::new()
            }
            Hit::SidebarPane => {
                self.focus = Focus::Sidebar;
                Vec::new()
            }
        }
    }

    fn right_click(&mut self, column: u16, row: u16) -> Vec<Command> {
        let Some(hit) = self.hitmap.hit(column, row) else {
            if self.overlay.is_some() {
                self.close_overlay();
            }
            return Vec::new();
        };
        if self.overlay.is_some() {
            if !matches!(hit, Hit::Overlay) {
                self.close_overlay();
            }
            return Vec::new();
        }
        let menu = match hit {
            Hit::TrackRow(index) => {
                self.playlist_selection = index;
                self.sync_selection();
                let Some(track) = self.selected_track().cloned() else {
                    return Vec::new();
                };
                ContextMenu {
                    x: column,
                    y: row,
                    title: track.title.clone(),
                    items: vec![
                        ContextItem {
                            label: "Play".to_string(),
                            action: ContextAction::PlayTrack(track.clone()),
                        },
                        ContextItem {
                            label: "Add to queue".to_string(),
                            action: ContextAction::QueueTrack(track.clone()),
                        },
                        ContextItem {
                            label: "Add to playlist".to_string(),
                            action: ContextAction::AddTrackToPlaylist(track),
                        },
                    ],
                    selected: 0,
                }
            }
            Hit::QueueRow(index) => {
                self.queue_selection = index;
                self.sync_selection();
                let Some(item) = self.queue.items.get(index) else {
                    return Vec::new();
                };
                let mut items = vec![
                    ContextItem {
                        label: "Play".to_string(),
                        action: ContextAction::PlayQueueIndex(index),
                    },
                    ContextItem {
                        label: "Remove".to_string(),
                        action: ContextAction::RemoveQueue(index),
                    },
                ];
                if index > 0 {
                    items.push(ContextItem {
                        label: "Move up".to_string(),
                        action: ContextAction::MoveQueueUp(index),
                    });
                }
                if index + 1 < self.queue.items.len() {
                    items.push(ContextItem {
                        label: "Move down".to_string(),
                        action: ContextAction::MoveQueueDown(index),
                    });
                    items.push(ContextItem {
                        label: "Play next".to_string(),
                        action: ContextAction::PlayQueueNext(index),
                    });
                }
                if matches!(item.download, DownloadStatus::Failed { .. }) {
                    items.push(ContextItem {
                        label: "Retry download".to_string(),
                        action: ContextAction::RetryDownload(item.track.video_id.clone()),
                    });
                }
                items.push(ContextItem {
                    label: "Clear queue".to_string(),
                    action: ContextAction::ClearQueue,
                });
                ContextMenu {
                    x: column,
                    y: row,
                    title: item.track.title.clone(),
                    items,
                    selected: 0,
                }
            }
            Hit::LibraryRow(index) | Hit::SidebarPlaylist(index) => {
                let Some(playlist) = self.playlists.get(index).cloned() else {
                    return Vec::new();
                };
                ContextMenu {
                    x: column,
                    y: row,
                    title: playlist.name.clone(),
                    items: vec![
                        ContextItem {
                            label: "Open".to_string(),
                            action: ContextAction::OpenPlaylist(playlist.id.clone()),
                        },
                        ContextItem {
                            label: "Play".to_string(),
                            action: ContextAction::PlayPlaylist(playlist.id.clone()),
                        },
                        ContextItem {
                            label: "Rename".to_string(),
                            action: ContextAction::RenamePlaylist(playlist.id.clone()),
                        },
                        ContextItem {
                            label: "Delete".to_string(),
                            action: ContextAction::DeletePlaylist(playlist.id),
                        },
                    ],
                    selected: 0,
                }
            }
            _ => return Vec::new(),
        };
        self.overlay = Some(Overlay::Context(menu));
        Vec::new()
    }

    fn scroll(&mut self, column: u16, row: u16, delta: isize) -> Vec<Command> {
        let hit = self.hitmap.hit(column, row);
        if self.overlay.is_some() {
            if matches!(hit, Some(Hit::Overlay) | Some(Hit::SearchRow(_))) {
                self.search.selected = clamp_index(
                    (self.search.selected as isize + delta) as usize,
                    self.search.len(),
                );
            }
            return Vec::new();
        }
        match hit {
            Some(Hit::SidebarPane | Hit::SidebarPlaylist(_) | Hit::Nav(_)) => {
                self.focus = Focus::Sidebar;
                self.move_selection(delta);
            }
            Some(_) => {
                self.focus = Focus::Main;
                self.move_selection(delta);
            }
            None => self.move_selection(delta),
        }
        Vec::new()
    }

    fn is_double_click(&mut self, column: u16, row: u16, hit: Hit) -> bool {
        let now = Instant::now();
        let double = self
            .last_click
            .map(|(at, last_column, last_row, last_hit)| {
                last_hit == hit
                    && now.duration_since(at) < DOUBLE_CLICK_TIMEOUT
                    && last_column.abs_diff(column) <= 1
                    && last_row.abs_diff(row) <= 1
            })
            .unwrap_or(false);
        self.last_click = Some((now, column, row, hit));
        double
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Sidebar => {
                self.sidebar_selection =
                    add_index(self.sidebar_selection, delta, self.playlists.len());
            }
            Focus::Main => match self.screen {
                Screen::Library => {
                    self.library_selection =
                        add_index(self.library_selection, delta, self.playlists.len());
                }
                Screen::Playlist => {
                    let len = self
                        .playlist
                        .as_ref()
                        .map(|playlist| playlist.tracks.len())
                        .unwrap_or(0);
                    self.playlist_selection = add_index(self.playlist_selection, delta, len);
                }
                Screen::Queue => {
                    self.queue_selection =
                        add_index(self.queue_selection, delta, self.queue.items.len());
                }
            },
        }
        self.sync_selection();
    }

    fn jump_first(&mut self) {
        match (self.focus, self.screen) {
            (Focus::Sidebar, _) => self.sidebar_selection = 0,
            (Focus::Main, Screen::Library) => self.library_selection = 0,
            (Focus::Main, Screen::Playlist) => self.playlist_selection = 0,
            (Focus::Main, Screen::Queue) => self.queue_selection = 0,
        }
        self.sync_selection();
    }

    fn jump_last(&mut self) {
        match (self.focus, self.screen) {
            (Focus::Sidebar, _) => {
                self.sidebar_selection = self.playlists.len().saturating_sub(1);
            }
            (Focus::Main, Screen::Library) => {
                self.library_selection = self.playlists.len().saturating_sub(1);
            }
            (Focus::Main, Screen::Playlist) => {
                self.playlist_selection = self
                    .playlist
                    .as_ref()
                    .map(|playlist| playlist.tracks.len().saturating_sub(1))
                    .unwrap_or(0);
            }
            (Focus::Main, Screen::Queue) => {
                self.queue_selection = self.queue.items.len().saturating_sub(1);
            }
        }
        self.sync_selection();
    }

    fn half_page(&mut self, direction: isize) {
        self.move_selection(direction * 8);
    }

    fn go_library(&mut self) {
        self.screen = Screen::Library;
        self.focus = Focus::Main;
        self.playlist = None;
        self.playlist_request = None;
        self.library_selection = clamp_index(self.library_selection, self.playlists.len());
        self.sync_selection();
    }

    fn go_queue(&mut self) {
        self.screen = Screen::Queue;
        self.focus = Focus::Main;
        self.playlist = None;
        self.playlist_request = None;
        self.queue_selection = clamp_index(self.queue_selection, self.queue.items.len());
        self.sync_selection();
    }

    fn open_playlist(&mut self, id: &str) -> Vec<Command> {
        self.screen = Screen::Playlist;
        self.focus = Focus::Main;
        self.playlist = None;
        self.playlist_request = Some(id.to_string());
        self.playlist_selection = 0;
        self.sync_selection();
        vec![Command::OpenPlaylist(id.to_string())]
    }

    fn activate(&mut self) -> Vec<Command> {
        if self.focus == Focus::Sidebar {
            return self
                .selected_playlist()
                .cloned()
                .map(|playlist| self.open_playlist(&playlist.id))
                .unwrap_or_default();
        }
        match self.screen {
            Screen::Library => self
                .selected_playlist()
                .cloned()
                .map(|playlist| self.open_playlist(&playlist.id))
                .unwrap_or_default(),
            Screen::Playlist => self
                .selected_track()
                .cloned()
                .map(|track| vec![Command::PlayTrack(track)])
                .unwrap_or_default(),
            Screen::Queue => self
                .queue
                .items
                .get(self.queue_selection)
                .map(|item| vec![Command::PlayTrack(item.track.clone())])
                .unwrap_or_default(),
        }
    }

    fn play_selected_playlist(&mut self) -> Vec<Command> {
        let id = match self.screen {
            Screen::Playlist => self.open_playlist_id(),
            _ => self.selected_playlist().map(|playlist| playlist.id.clone()),
        };
        id.map(|id| vec![Command::PlayPlaylist(id)])
            .unwrap_or_default()
    }

    fn queue_selected(&mut self) -> Vec<Command> {
        if self.focus == Focus::Sidebar {
            return Vec::new();
        }
        self.selected_track()
            .cloned()
            .map(|track| {
                self.show_toast(ToastLevel::Info, format!("Queued {}.", track.title));
                vec![Command::QueueTrack(track)]
            })
            .unwrap_or_default()
    }

    fn add_selected_to_playlist(&mut self) -> Vec<Command> {
        if self.focus == Focus::Sidebar {
            return Vec::new();
        }
        let Some(track) = self.selected_track().cloned() else {
            return Vec::new();
        };
        self.overlay = Some(Overlay::PickPlaylist(PickPlaylistModal {
            track,
            selected: 0,
        }));
        Vec::new()
    }

    fn retry_selected(&mut self) -> Vec<Command> {
        if self.focus == Focus::Sidebar || self.screen != Screen::Queue {
            return Vec::new();
        }
        self.queue
            .items
            .get(self.queue_selection)
            .map(|item| vec![Command::RetryDownload(item.track.video_id.clone())])
            .unwrap_or_default()
    }

    fn delete_selected(&mut self) -> Vec<Command> {
        if self.focus == Focus::Sidebar {
            return self
                .selected_playlist()
                .cloned()
                .map(|playlist| {
                    self.overlay = Some(Overlay::Confirm(ConfirmModal {
                        title: "Delete playlist".to_string(),
                        message: format!("Delete {}? This cannot be undone.", playlist.name),
                        action: ConfirmAction::DeletePlaylist(playlist.id),
                    }));
                    Vec::new()
                })
                .unwrap_or_default();
        }
        match self.screen {
            Screen::Library => self
                .selected_playlist()
                .cloned()
                .map(|playlist| {
                    self.overlay = Some(Overlay::Confirm(ConfirmModal {
                        title: "Delete playlist".to_string(),
                        message: format!("Delete {}? This cannot be undone.", playlist.name),
                        action: ConfirmAction::DeletePlaylist(playlist.id),
                    }));
                    Vec::new()
                })
                .unwrap_or_default(),
            Screen::Playlist => {
                let Some(id) = self.open_playlist_id() else {
                    return Vec::new();
                };
                if let Some(track) = self.selected_track().cloned() {
                    self.overlay = Some(Overlay::Confirm(ConfirmModal {
                        title: "Remove track".to_string(),
                        message: format!("Remove {} from the playlist?", track.title),
                        action: ConfirmAction::RemovePlaylistTrack {
                            id,
                            index: self.playlist_selection,
                        },
                    }));
                }
                Vec::new()
            }
            Screen::Queue => {
                if let Some(item) = self.queue.items.get(self.queue_selection) {
                    self.overlay = Some(Overlay::Confirm(ConfirmModal {
                        title: "Remove track".to_string(),
                        message: format!("Remove {} from the queue?", item.track.title),
                        action: ConfirmAction::RemoveQueue(self.queue_selection),
                    }));
                }
                Vec::new()
            }
        }
    }

    fn clear_queue_confirmation(&mut self) -> Vec<Command> {
        if self.screen != Screen::Queue || self.queue.items.is_empty() {
            return Vec::new();
        }
        self.overlay = Some(Overlay::Confirm(ConfirmModal {
            title: "Clear queue".to_string(),
            message: "Remove all tracks from the queue?".to_string(),
            action: ConfirmAction::ClearQueue,
        }));
        Vec::new()
    }

    fn move_item(&mut self, delta: isize) -> Vec<Command> {
        if self.focus == Focus::Sidebar {
            return Vec::new();
        }
        match self.screen {
            Screen::Playlist => {
                let Some(id) = self.open_playlist_id() else {
                    return Vec::new();
                };
                let len = self
                    .playlist
                    .as_ref()
                    .map(|playlist| playlist.tracks.len())
                    .unwrap_or(0);
                let Some(to) = moved_index(self.playlist_selection, delta, len) else {
                    return Vec::new();
                };
                let from = self.playlist_selection;
                self.playlist_selection = to;
                self.sync_selection();
                vec![Command::MovePlaylistTrack { id, from, to }]
            }
            Screen::Queue => {
                let Some(to) = moved_index(self.queue_selection, delta, self.queue.items.len())
                else {
                    return Vec::new();
                };
                let from = self.queue_selection;
                self.queue_selection = to;
                self.sync_selection();
                self.reorder_queue(from, to)
            }
            Screen::Library => Vec::new(),
        }
    }

    fn reorder_queue(&mut self, from: usize, to: usize) -> Vec<Command> {
        if from == to || to >= self.queue.items.len() {
            return Vec::new();
        }
        self.queue_selection = to;
        self.sync_selection();
        vec![Command::QueueReorder { from, to }]
    }

    fn open_create_playlist(&mut self) {
        self.overlay = Some(Overlay::Input(InputModal {
            title: "New playlist".to_string(),
            value: String::new(),
            action: InputAction::CreatePlaylist,
        }));
    }

    fn open_rename_playlist(&mut self) -> Vec<Command> {
        let playlist = match (self.focus, self.screen) {
            (Focus::Main, Screen::Playlist) => self
                .playlist
                .as_ref()
                .map(|playlist| playlist.playlist.clone()),
            _ => self.selected_playlist().cloned(),
        };
        let Some(playlist) = playlist else {
            return Vec::new();
        };
        self.overlay = Some(Overlay::Input(InputModal {
            title: "Rename playlist".to_string(),
            value: playlist.name,
            action: InputAction::RenamePlaylist(playlist.id),
        }));
        Vec::new()
    }

    fn toggle_playback(&mut self) -> Vec<Command> {
        if self.current_track().is_none() {
            return Vec::new();
        }
        if self.is_playing() {
            vec![Command::Pause]
        } else {
            vec![Command::Resume]
        }
    }

    fn next_track(&mut self) -> Vec<Command> {
        vec![Command::Next]
    }

    fn previous_track(&mut self) -> Vec<Command> {
        vec![Command::Previous]
    }

    fn adjust_volume(&mut self, delta: i16) -> Vec<Command> {
        let current = self
            .status
            .as_ref()
            .map(|status| i16::from(status.playback.volume))
            .unwrap_or(50);
        let volume = (current + delta).clamp(0, 100) as u8;
        vec![Command::Volume(volume)]
    }

    fn toggle_shuffle(&mut self) -> Vec<Command> {
        let shuffle = self
            .status
            .as_ref()
            .map(|status| status.playback.shuffle)
            .unwrap_or(false);
        vec![Command::Shuffle(!shuffle)]
    }

    fn cycle_repeat(&mut self) -> Vec<Command> {
        let repeat = self
            .status
            .as_ref()
            .map(|status| status.playback.repeat)
            .unwrap_or(RepeatMode::Off);
        vec![Command::Repeat(crate::daemon::next_repeat(repeat))]
    }

    fn seek_relative(&mut self, delta: f64) -> Vec<Command> {
        if self.current_track().is_none() {
            return Vec::new();
        }
        let mut position = self.position_secs() + delta;
        if let Some(duration) = self.duration_secs() {
            position = position.min(duration);
        }
        vec![Command::Seek(position.max(0.0))]
    }

    fn show_toast(&mut self, level: ToastLevel, message: impl Into<String>) {
        self.toasts.push(Toast {
            level,
            message: message.into(),
            created: Instant::now(),
        });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }
}

/// Parse a `M:SS` or `H:MM:SS` duration.
pub fn parse_duration(value: &str) -> Option<f64> {
    let mut seconds = 0.0;
    let mut parts = 0;
    for part in value.split(':') {
        let part: f64 = part.trim().parse().ok()?;
        seconds = seconds * 60.0 + part;
        parts += 1;
    }
    if parts == 0 { None } else { Some(seconds) }
}

fn is_char(key: KeyEvent, character: char) -> bool {
    matches!(key.code, KeyCode::Char(value) if value == character)
}

fn is_ctrl(key: KeyEvent, character: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && is_char(key, character)
}

fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

fn add_index(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let value = index as isize + delta;
    value.clamp(0, len as isize - 1) as usize
}

fn moved_index(index: usize, delta: isize, len: usize) -> Option<usize> {
    let target = index as isize + delta;
    if target < 0 || target >= len as isize {
        None
    } else {
        Some(target as usize)
    }
}

fn fraction(column: u16, rect: ratatui::layout::Rect) -> f64 {
    if rect.width == 0 {
        return 0.0;
    }
    ((column.saturating_sub(rect.x)) as f64 / f64::from(rect.width)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use ratatui::layout::Rect;
    use ytermusicd_api::{AuthStatus, DownloadSummary, PlaybackSnapshot, QueueItem};

    fn track(id: &str) -> Track {
        Track {
            video_id: id.to_string(),
            title: format!("Track {id}"),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
        }
    }

    fn status() -> Status {
        Status {
            name: "ytermusicd".to_string(),
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
                current: Some(track("v1")),
                position_secs: 10.0,
                volume: 50,
                repeat: RepeatMode::Off,
                shuffle: false,
            },
            queue_length: 1,
            downloads: DownloadSummary::default(),
        }
    }

    fn queue() -> Queue {
        Queue {
            items: vec![QueueItem {
                track: track("v1"),
                download: DownloadStatus::Downloaded,
            }],
            current: Some(0),
            repeat: RepeatMode::Off,
            shuffle: false,
        }
    }

    fn playlists() -> Vec<Playlist> {
        vec![
            Playlist {
                id: "p1".to_string(),
                name: "One".to_string(),
                subtitle: "First".to_string(),
            },
            Playlist {
                id: "p2".to_string(),
                name: "Two".to_string(),
                subtitle: "Second".to_string(),
            },
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn snapshot_applies_state_and_requests_a_cover() {
        let mut app = App::new("http://localhost");
        let commands = app.handle_daemon_event(DaemonEvent::Snapshot {
            status: status(),
            queue: queue(),
        });
        assert_eq!(app.status.as_ref().unwrap().name, "ytermusicd");
        assert_eq!(app.queue.items.len(), 1);
        assert_eq!(app.connection, Connection::Online);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::LoadCover(id) if id == "v1"))
        );
    }

    #[test]
    fn playback_event_requests_a_cover_for_a_new_track() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Snapshot {
            status: status(),
            queue: queue(),
        });
        let mut playback = status().playback;
        playback.current = Some(track("v2"));
        let commands = app.handle_daemon_event(DaemonEvent::Playback(playback));
        assert!(matches!(commands.first(), Some(Command::LoadCover(id)) if id == "v2"));
    }

    #[test]
    fn failed_cover_fetch_retries_on_the_next_playback_event() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Snapshot {
            status: status(),
            queue: queue(),
        });
        let commands = app.handle_daemon_event(DaemonEvent::Cover {
            video_id: "v1".to_string(),
            art: None,
        });
        assert!(commands.is_empty());
        let playback = status().playback;
        let commands = app.handle_daemon_event(DaemonEvent::Playback(playback));
        assert!(matches!(commands.first(), Some(Command::LoadCover(id)) if id == "v1"));
    }

    #[test]
    fn search_debounce_emits_one_query() {
        let mut app = App::new("http://localhost");
        app.overlay = Some(Overlay::Search);
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('b')));
        app.search.send_at = Some(Instant::now());
        let commands = app.tick();
        assert!(matches!(commands.first(), Some(Command::Search(query)) if query == "ab"));
        assert_eq!(app.search.query, "ab");
    }

    #[test]
    fn gg_moves_to_the_first_playlist() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Playlists(playlists()));
        app.library_selection = 1;
        app.handle_key(key(KeyCode::Char('g')));
        app.handle_key(key(KeyCode::Char('g')));
        assert_eq!(app.library_selection, 0);
    }

    #[test]
    fn dd_opens_the_delete_confirmation() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Playlists(playlists()));
        app.handle_key(key(KeyCode::Char('d')));
        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(ConfirmModal {
                action: ConfirmAction::DeletePlaylist(_),
                ..
            }))
        ));
    }

    #[test]
    fn tab_switches_focus() {
        let mut app = App::new("http://localhost");
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Focus::Main);
    }

    #[test]
    fn double_click_plays_a_playlist_track() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Playlists(playlists()));
        let detail = PlaylistDetail {
            playlist: playlists()[0].clone(),
            tracks: vec![track("v1"), track("v2")],
        };
        app.screen = Screen::Playlist;
        app.playlist = Some(detail);
        app.sync_selection();
        app.hitmap.push(Rect::new(0, 5, 40, 1), Hit::TrackRow(1));
        let first = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 5));
        assert!(first.is_empty());
        let second = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 5));
        assert!(
            matches!(second.first(), Some(Command::PlayTrack(track)) if track.video_id == "v2")
        );
    }

    #[test]
    fn enter_opens_the_sidebar_playlist() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Playlists(playlists()));
        app.focus = Focus::Sidebar;
        app.sidebar_selection = 1;
        let commands = app.handle_key(key(KeyCode::Enter));
        assert!(matches!(
            commands.first(),
            Some(Command::OpenPlaylist(id)) if id == "p2"
        ));
        assert_eq!(app.screen, Screen::Playlist);
    }

    #[test]
    fn a_click_outside_an_overlay_closes_it() {
        let mut app = App::new("http://localhost");
        app.overlay = Some(Overlay::Help);
        app.hitmap.push(Rect::new(0, 0, 20, 10), Hit::MainPane);
        app.hitmap.push(Rect::new(5, 2, 10, 5), Hit::Overlay);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 1, 1));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn queue_reorder_key_emits_a_command() {
        let mut app = App::new("http://localhost");
        app.screen = Screen::Queue;
        app.queue.items = vec![
            QueueItem {
                track: track("v1"),
                download: DownloadStatus::Downloaded,
            },
            QueueItem {
                track: track("v2"),
                download: DownloadStatus::Downloaded,
            },
        ];
        app.sync_selection();
        let commands = app.handle_key(key(KeyCode::Char('J')));
        assert!(matches!(
            commands.first(),
            Some(Command::QueueReorder { from: 0, to: 1 })
        ));
    }

    #[test]
    fn stale_search_results_are_ignored() {
        let mut app = App::new("http://localhost");
        app.search.query = "new".to_string();
        let results = SearchResults {
            tracks: vec![track("v1")],
            playlists: Vec::new(),
        };
        app.handle_daemon_event(DaemonEvent::Search {
            query: "old".to_string(),
            results,
        });
        assert!(app.search.results.tracks.is_empty());
    }

    #[test]
    fn a_playlist_error_returns_to_the_library() {
        let mut app = App::new("http://localhost");
        app.screen = Screen::Playlist;
        app.playlist_request = Some("p1".to_string());
        app.handle_daemon_event(DaemonEvent::PlaylistError {
            id: "p1".to_string(),
            message: "not found".to_string(),
        });
        assert_eq!(app.screen, Screen::Library);
        assert!(app.playlist.is_none());
        assert_eq!(app.toasts.len(), 1);
    }

    #[test]
    fn a_search_error_clears_the_loading_state() {
        let mut app = App::new("http://localhost");
        app.search.query = "neon".to_string();
        app.search.loading = true;
        app.handle_daemon_event(DaemonEvent::SearchError {
            query: "neon".to_string(),
            message: "failed".to_string(),
        });
        assert!(!app.search.loading);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("3:24"), Some(204.0));
        assert_eq!(parse_duration("1:02:03"), Some(3723.0));
        assert_eq!(parse_duration("bad"), None);
    }

    #[test]
    fn downloaded_status_is_visible_in_the_queue() {
        let mut app = App::new("http://localhost");
        app.handle_daemon_event(DaemonEvent::Snapshot {
            status: status(),
            queue: queue(),
        });
        assert_eq!(app.queue.items[0].download, DownloadStatus::Downloaded);
        assert_eq!(app.queue_state.selected(), Some(0));
    }
}
