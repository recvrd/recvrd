use clap::{Parser, Subcommand};
use common_structs::ProviderId;
use recvrd_api::{CacheCleanRequest, CacheCleanResult, CacheScope, MusicApi};
use recvrd_core::{
    AuthStatus, DaemonConfig, Downloader as ConfigDownloader, Paths, PersistedState,
    PlaybackService, is_loopback_bind, resolve_auth,
};
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

mod artwork;
mod audio;
mod cache;
mod downloads;
mod mcp;
mod mpris;
mod music;
mod providers;
mod server;
mod web;

use crate::{
    artwork::ArtworkCache,
    audio::RodioSink,
    cache::CacheService,
    downloads::{DownloadControl, DownloadService},
    music::UnavailableMusicApi,
    providers::{CompositeCoverFetcher, CompositeDownloader, CompositeMusicApi},
};
use provider_podverse::PodverseProvider;
use provider_rss::RssProvider;
use provider_youtube::{
    AUTH_EXPIRED_MESSAGE, YoutubeCoverFetcher, YoutubeDownloader, YoutubeMusicApi,
    YoutubeMusicError, connect,
};

/// Old environment names and their new names.
const LEGACY_ENV: [(&str, &str); 6] = [
    ("RECVRD_BIND", "YTERMUSICD_BIND"),
    ("RECVRD_PORT", "YTERMUSICD_PORT"),
    ("RECVRD_HEADERS", "YTERMUSICD_HEADERS"),
    ("RECVRD_CONFIG", "YTERMUSICD_CONFIG"),
    ("RECVRD_URL", "YTERMUSICD_URL"),
    ("RECVRD_DEBUG", "YTERMUSICD_DEBUG"),
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

#[derive(Debug, Parser)]
#[command(
    name = "recvrd",
    version,
    about = "Headless music daemon with a web UI, MPRIS and MCP"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the headless daemon
    Serve {
        /// Address to bind the HTTP API to
        #[arg(long, env = "RECVRD_BIND")]
        bind: Option<String>,
        /// Port for the HTTP API
        #[arg(long, env = "RECVRD_PORT")]
        port: Option<u16>,
        /// Path to the headers.txt file (Cookie + User-Agent)
        #[arg(long, env = "RECVRD_HEADERS")]
        headers: Option<PathBuf>,
        /// Path to config.toml
        #[arg(long, env = "RECVRD_CONFIG")]
        config: Option<PathBuf>,
    },
    /// Run the stdio MCP proxy
    Mcp {
        /// Base URL of the running daemon
        #[arg(long, env = "RECVRD_URL")]
        url: Option<String>,
        /// Include the endpoint and the raw daemon response (truncated)
        /// in invalid-response errors, instead of only the serde error.
        /// Also settable with `RECVRD_DEBUG=true`.
        #[arg(long, env = "RECVRD_DEBUG")]
        debug: bool,
    },
    /// Manage the daemon cache
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
}

/// Cache actions.
#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// Delete cached downloads and cover art
    Clean {
        /// What to delete
        #[arg(long, value_enum, default_value_t = CliCacheScope::All)]
        scope: CliCacheScope,
        /// Base URL of the running daemon
        #[arg(long, env = "RECVRD_URL")]
        url: Option<String>,
    },
}

/// Cache scope for the `cache clean` command.
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
enum CliCacheScope {
    Downloads,
    Covers,
    #[default]
    All,
}

impl CliCacheScope {
    fn api_scope(self) -> CacheScope {
        match self {
            Self::Downloads => CacheScope::Downloads,
            Self::Covers => CacheScope::Covers,
            Self::All => CacheScope::All,
        }
    }
}

fn main() -> ExitCode {
    apply_legacy_env();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve {
            bind,
            port,
            headers,
            config,
        } => match run_serve(bind, port, headers, config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("recvrd serve failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Mcp { url, debug } => match run_mcp(url, debug) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("recvrd mcp failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Cache { command } => match command {
            CacheCommand::Clean { scope, url } => match run_cache_clean(scope, url) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("recvrd cache clean failed: {error}");
                    ExitCode::FAILURE
                }
            },
        },
    }
}

fn run_mcp(url: Option<String>, debug: bool) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(mcp::run(url, debug))
}

/// Clean the cache. When the daemon answers, the daemon cleans it:
/// it stops active downloads, keeps the current track, and resets
/// the queue statuses. When the daemon does not answer, the files
/// are cleaned directly from disk instead.
fn run_cache_clean(scope: CliCacheScope, url: Option<String>) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(cache_clean(scope, url))
}

async fn cache_clean(scope: CliCacheScope, url: Option<String>) -> std::io::Result<()> {
    let url = url.unwrap_or_else(|| "http://127.0.0.1:8080".to_string());
    let api_url = format!("{}/api/cache/clean", url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let request = CacheCleanRequest {
        scope: scope.api_scope(),
    };
    match client.post(&api_url).json(&request).send().await {
        Ok(response) => {
            if response.status().is_success() {
                let result: CacheCleanResult = response.json().await.map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
                })?;
                print_clean_result(&result);
                Ok(())
            } else {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Err(std::io::Error::other(format!(
                    "the daemon refused the clean: {status} {body}"
                )))
            }
        }
        Err(_) => {
            eprintln!(
                "The daemon did not answer at {url}. The files are cleaned \
                 directly. Active downloads are not stopped, so stop the \
                 daemon first for a full clean."
            );
            clean_cache_on_disk(scope.api_scope())
        }
    }
}

/// Clean the cache files directly from disk. The current track from
/// the saved playback state is kept when the state names one.
fn clean_cache_on_disk(scope: CacheScope) -> std::io::Result<()> {
    let paths = Paths::discover()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::NotFound, error.to_string()))?;
    paths.ensure()?;
    let keep = PersistedState::load(&paths.state_file())
        .ok()
        .and_then(|state| {
            state
                .current
                .and_then(|index| state.tracks.get(index))
                .map(|track| track.key())
        });
    let cache = CacheService::new(paths.cache_dir.join("downloads"), paths.covers_dir());
    let result = cache.clean(scope, keep.as_ref());
    print_clean_result(&result);
    Ok(())
}

/// Print the freed bytes and deleted file counts.
fn print_clean_result(result: &CacheCleanResult) {
    println!(
        "Cache cleaned: freed {} ({} {}, {} {}).",
        format_bytes(result.freed_bytes),
        result.downloads_deleted_files,
        plural(result.downloads_deleted_files, "download file"),
        result.covers_deleted_files,
        plural(result.covers_deleted_files, "cover file"),
    );
}

/// The singular or plural form of a file count noun.
fn plural(count: u64, singular: &str) -> String {
    if count == 1 {
        singular.to_string()
    } else {
        format!("{singular}s")
    }
}

/// Format a byte count for display.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn run_serve(
    bind: Option<String>,
    port: Option<u16>,
    headers: Option<PathBuf>,
    config: Option<PathBuf>,
) -> std::io::Result<()> {
    init_logging();

    let paths = Paths::discover()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::NotFound, error.to_string()))?;
    paths.ensure()?;

    let config_path = config.clone().unwrap_or_else(|| paths.config_file());
    let mut config = match config {
        Some(path) => DaemonConfig::load(&path),
        None => DaemonConfig::load_or_default(&config_path),
    }
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()))?;
    if let Some(value) = std::env::var("RECVRD_PODVERSE_URL")
        .ok()
        .or_else(|| std::env::var("YTERMUSICD_PODVERSE_URL").ok())
    {
        config.podverse.base_url = Some(value);
    }
    if let Some(value) = std::env::var("RECVRD_PODVERSE_TOKEN")
        .ok()
        .or_else(|| std::env::var("YTERMUSICD_PODVERSE_TOKEN").ok())
    {
        config.podverse.token = Some(value);
    }

    let bind = bind
        .or(config.bind)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.or(config.port).unwrap_or(8080);
    let headers = headers.or(config.headers);

    let (mut auth_status, auth, auth_source) = match resolve_auth(&paths, headers.as_deref()) {
        Ok(loaded) => {
            log::info!("loaded auth from {}", loaded.source.display());
            if loaded.legacy {
                log::warn!(
                    "legacy headers path in use; move it to {}",
                    paths.headers_file().display()
                );
            }
            let source = loaded.source.clone();
            (
                AuthStatus::ok(loaded.source),
                Some(loaded.auth),
                Some(source),
            )
        }
        Err(error) => {
            log::error!("{error}");
            eprintln!("{error}");
            eprintln!(
                "Create a headers.txt with `Cookie:` and `User-Agent:` lines, \
                 or pass `--headers <path>`. See README.md#authentication."
            );
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                error.to_string(),
            ));
        }
    };

    let dbus = config.dbus;
    let parallel_downloads = config.parallel_downloads;
    let cover_cache_bytes = config.cover_cache_mb.saturating_mul(1024 * 1024);
    let downloader = match config.downloader {
        ConfigDownloader::Ytdlp => YoutubeDownloader::new(),
        ConfigDownloader::RustyYtdl => {
            log::warn!("rusty-ytdl backend requested but not compiled; using yt-dlp");
            YoutubeDownloader::new()
        }
    };
    let cache_dir = paths.cache_dir.clone();
    let covers_dir = paths.covers_dir();
    let state_path = paths.state_file();
    if let Err(error) = migrate_cache(&cache_dir, &covers_dir) {
        log::error!("cache migration failed: {error}");
        eprintln!("cache migration failed: {error}");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
    }
    log::info!("starting recvrd serve on {bind}:{port}");
    if !is_loopback_bind(&bind) {
        let warning = format!(
            "WARNING: bind address {bind} is not loopback. The daemon API has no \
             authentication, so any host that can reach {bind}:{port} can control playback \
             and playlists. Bind 127.0.0.1 and use a reverse proxy for remote access."
        );
        log::warn!("{warning}");
        eprintln!("{warning}");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let rss = Arc::new(RssProvider::new(paths.rss_feeds_file()));
        for (id, result) in rss.refresh_all().await {
            match result {
                Ok(feed) => log::info!(
                    "refreshed RSS feed {} ({} episodes)",
                    feed.title,
                    feed.episode_count
                ),
                Err(error) => log::warn!("could not refresh RSS feed {id}: {error}"),
            }
        }

        let podverse = match config.podverse.base_url.as_deref() {
            Some(base_url) => match PodverseProvider::new(&config.podverse) {
                Ok(provider) => {
                    log::info!("Podverse provider ready for {base_url}");
                    Some(Arc::new(provider))
                }
                Err(error) => {
                    log::error!("Podverse provider disabled: {error}");
                    None
                }
            },
            None => None,
        };
        let mut providers = vec![ProviderId::Youtube, ProviderId::Rss];
        if podverse.is_some() {
            providers.push(ProviderId::Podverse);
        }

        let music: Arc<dyn MusicApi> = match auth {
            Some(auth) => match connect(&auth).await {
                Ok(client) => {
                    log::info!("YouTube Music client ready");
                    let source = auth_source.as_ref().map(|path| path.display().to_string());
                    Arc::new(CompositeMusicApi::new(
                        Arc::new(YoutubeMusicApi::new(client, source)),
                        rss.clone(),
                        podverse.clone(),
                    ))
                }
                Err(error) => {
                    log::error!("could not create YouTube Music client: {error:?}");
                    if matches!(error, YoutubeMusicError::NeedToLogin) {
                        auth_status = AuthStatus::expired(
                            auth_source.clone().unwrap_or_else(|| paths.headers_file()),
                            AUTH_EXPIRED_MESSAGE,
                        );
                    }
                    Arc::new(CompositeMusicApi::new(
                        Arc::new(UnavailableMusicApi),
                        rss.clone(),
                        podverse.clone(),
                    ))
                }
            },
            None => Arc::new(CompositeMusicApi::new(
                Arc::new(UnavailableMusicApi),
                rss.clone(),
                podverse.clone(),
            )),
        };

        let sink = RodioSink::spawn(config.initial_volume);
        let playback = Arc::new(AsyncMutex::new(PlaybackService::new(
            sink,
            cache_dir.clone(),
            config.initial_volume,
        )));
        {
            let mut service = playback.lock().await;
            match service.load_state(&state_path) {
                Ok(()) => log::info!("restored playback state from {}", state_path.display()),
                Err(error) => {
                    log::warn!(
                        "could not restore {}: {error}; starting with empty state",
                        state_path.display()
                    );
                    let backup = state_path.with_extension("json.bad");
                    let _ = std::fs::rename(&state_path, backup);
                }
            }
            service.set_wait_for_downloads(true);
        }

        let downloads = DownloadService::new(
            cache_dir,
            parallel_downloads,
            Arc::new(CompositeDownloader::new(
                Arc::new(downloader),
                rss.clone(),
                podverse.clone(),
            )),
        );
        downloads.spawn(playback.clone());
        let restored_tracks: Vec<recvrd_core::Track> = playback
            .lock()
            .await
            .queue_snapshot()
            .items
            .into_iter()
            .map(|item| item.track)
            .collect();
        if !restored_tracks.is_empty() {
            downloads.queue_playlist(&restored_tracks);
        }
        let downloads: Arc<dyn DownloadControl> = Arc::new(downloads);
        let artwork = Arc::new(ArtworkCache::new(
            covers_dir.clone(),
            cover_cache_bytes,
            Arc::new(CompositeCoverFetcher::new(
                Arc::new(YoutubeCoverFetcher::new()),
                rss.clone(),
                podverse,
            )),
        ));
        let cache = Arc::new(CacheService::new(
            paths.cache_dir.join("downloads"),
            covers_dir,
        ));

        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let command_playback = playback.clone();
        let command_state_path = state_path.clone();
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                let mut service = command_playback.lock().await;
                if let Err(error) = service.handle_command(command) {
                    log::warn!("playback command failed: {error:?}");
                }
                if let Err(error) = service.save_state(&command_state_path) {
                    log::warn!("could not save playback state: {error}");
                }
            }
        });

        let tick_playback = playback.clone();
        let tick_state_path = state_path.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            let mut ticks = 0u32;
            loop {
                interval.tick().await;
                let mut service = tick_playback.lock().await;
                if let Err(error) = service.tick() {
                    log::warn!("playback tick failed: {error:?}");
                }
                ticks = ticks.wrapping_add(1);
                if ticks.is_multiple_of(10)
                    && let Err(error) = service.save_state(&tick_state_path)
                {
                    log::warn!("could not save playback state: {error}");
                }
            }
        });

        let serve_playback = playback.clone();
        let result = server::serve(
            &bind,
            port,
            auth_status,
            music,
            serve_playback,
            downloads,
            artwork,
            cache,
            commands,
            dbus,
            rss,
            providers,
        )
        .await;
        if let Err(error) = playback.lock().await.save_state(&state_path) {
            log::warn!("could not save playback state on shutdown: {error}");
        }
        result
    })
}

/// Move a version 0 cache into the provider-scoped layout, and
/// upgrade the database to version 1. Flat download and cover files
/// move to `downloads/youtube/` and `covers/youtube/`. A file that
/// cannot move stays in place and is logged, so a bad cache never
/// blocks startup. A database with an unknown version is an error.
fn migrate_cache(cache_dir: &Path, covers_dir: &Path) -> Result<(), String> {
    let database = database::YTLocalDatabase::new(cache_dir.to_path_buf());
    database.upgrade().map_err(|error| error.to_string())?;
    migrate_flat_files(
        &cache_dir.join("downloads"),
        &["mp4", "json", "part"],
        cache_dir,
    );
    migrate_flat_files(covers_dir, &["jpg"], cache_dir);
    Ok(())
}

/// Move flat files in `dir` into the `youtube` subdirectory. Files
/// with an invalid id go to quarantine, not to the trash.
fn migrate_flat_files(dir: &Path, extensions: &[&str], cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let scoped = dir.join(ProviderId::Youtube.as_str());
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !extensions.contains(&extension) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if !ProviderId::Youtube.validate_id(stem) {
            quarantine(&path, cache_dir);
            continue;
        }
        if std::fs::create_dir_all(&scoped).is_err() {
            log::warn!("could not create {}", scoped.display());
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        move_or_quarantine(&path, &scoped.join(name), cache_dir);
    }
}

/// Move `source` to `target`. When `target` exists, keep the larger
/// file and quarantine the smaller one.
fn move_or_quarantine(source: &Path, target: &Path, cache_dir: &Path) {
    if !target.exists() {
        if let Err(error) = std::fs::rename(source, target) {
            log::warn!(
                "could not move {} to {}: {error}",
                source.display(),
                target.display()
            );
        }
        return;
    }
    let source_len = std::fs::metadata(source)
        .map(|data| data.len())
        .unwrap_or(0);
    let target_len = std::fs::metadata(target)
        .map(|data| data.len())
        .unwrap_or(0);
    if source_len > target_len {
        quarantine(target, cache_dir);
        if let Err(error) = std::fs::rename(source, target) {
            log::warn!(
                "could not move {} to {}: {error}",
                source.display(),
                target.display()
            );
        }
    } else {
        quarantine(source, cache_dir);
    }
}

/// Move `path` to `cache/quarantine/`. A name clash adds a number.
fn quarantine(path: &Path, cache_dir: &Path) {
    let dir = cache_dir.join("quarantine");
    if std::fs::create_dir_all(&dir).is_err() {
        log::warn!("could not create {}", dir.display());
        return;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    let mut target = dir.join(name);
    let mut suffix = 1;
    while target.exists() {
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("file");
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        target = dir.join(format!("{stem}-{suffix}.{extension}"));
        suffix += 1;
    }
    if let Err(error) = std::fs::rename(path, &target) {
        log::warn!("could not quarantine {}: {error}", path.display());
    }
}

fn init_logging() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn write_file(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn migrate_cache_moves_flat_files_into_the_youtube_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        let covers = cache.join("covers");
        write_file(&cache.join("downloads/abcdefghijk.mp4"), b"video");
        write_file(&cache.join("downloads/abcdefghijk.json"), b"{}");
        write_file(&cache.join("downloads/abcdefghijk.part"), b"part");
        write_file(&covers.join("abcdefghijk.jpg"), b"jpeg");
        write_file(&cache.join("downloads/notes.txt"), b"keep me");

        migrate_cache(cache, &covers).unwrap();

        assert!(cache.join("downloads/youtube/abcdefghijk.mp4").exists());
        assert!(cache.join("downloads/youtube/abcdefghijk.json").exists());
        assert!(cache.join("downloads/youtube/abcdefghijk.part").exists());
        assert!(covers.join("youtube/abcdefghijk.jpg").exists());
        assert!(cache.join("downloads/notes.txt").exists());
        assert!(!cache.join("quarantine").exists());
    }

    #[test]
    fn migrate_cache_quarantines_files_with_invalid_ids() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        let covers = cache.join("covers");
        write_file(&cache.join("downloads/short.mp4"), b"video");
        write_file(&covers.join("not-an-id.jpg"), b"jpeg");

        migrate_cache(cache, &covers).unwrap();

        assert!(cache.join("quarantine/short.mp4").exists());
        assert!(cache.join("quarantine/not-an-id.jpg").exists());
        assert!(!cache.join("downloads/youtube").exists());
        assert!(!covers.join("youtube").exists());
    }

    #[test]
    fn migrate_cache_keeps_the_larger_file_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        let covers = cache.join("covers");
        // Flat file is smaller than the scoped copy: flat is quarantined.
        write_file(&cache.join("downloads/abcdefghijk.mp4"), b"small");
        write_file(
            &cache.join("downloads/youtube/abcdefghijk.mp4"),
            b"much larger scoped copy",
        );
        // Flat file is larger than the scoped copy: scoped is quarantined.
        write_file(
            &cache.join("downloads/zzzzzzzzzzz.mp4"),
            b"much larger flat copy",
        );
        write_file(&cache.join("downloads/youtube/zzzzzzzzzzz.mp4"), b"small");

        migrate_cache(cache, &covers).unwrap();

        assert_eq!(
            std::fs::read(cache.join("downloads/youtube/abcdefghijk.mp4")).unwrap(),
            b"much larger scoped copy"
        );
        assert!(cache.join("quarantine/abcdefghijk.mp4").exists());
        assert_eq!(
            std::fs::read(cache.join("downloads/youtube/zzzzzzzzzzz.mp4")).unwrap(),
            b"much larger flat copy"
        );
        assert!(cache.join("quarantine/zzzzzzzzzzz.mp4").exists());
    }

    #[test]
    fn migrate_cache_rejects_an_unknown_database_version() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path();
        let covers = cache.join("covers");
        write_file(&cache.join("db.bin"), b"YTDB\x63");

        let error = migrate_cache(cache, &covers).unwrap_err();
        assert!(error.contains("unsupported database version 99"), "{error}");
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_serve() {
        let cli = Cli::try_parse_from(["recvrd", "serve"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Serve {
                bind: None,
                port: None,
                headers: None,
                config: None
            }
        ));
    }

    #[test]
    fn parses_serve_options() {
        let cli = Cli::try_parse_from([
            "recvrd",
            "serve",
            "--bind",
            "0.0.0.0",
            "--port",
            "9000",
            "--headers",
            "/tmp/headers.txt",
            "--config",
            "/tmp/config.toml",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Serve {
                bind: Some(bind),
                port: Some(9000),
                headers: Some(headers),
                config: Some(config),
            } if bind == "0.0.0.0"
                && headers.as_path() == std::path::Path::new("/tmp/headers.txt")
                && config.as_path() == std::path::Path::new("/tmp/config.toml")
        ));
    }

    #[test]
    fn parses_mcp() {
        let cli = Cli::try_parse_from(["recvrd", "mcp"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Mcp {
                url: None,
                debug: false
            }
        ));
    }

    #[test]
    fn parses_mcp_url() {
        let cli = Cli::try_parse_from(["recvrd", "mcp", "--url", "http://127.0.0.1:9000"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Mcp { url: Some(url), .. } if url == "http://127.0.0.1:9000"
        ));
    }

    #[test]
    fn parses_mcp_debug() {
        let cli = Cli::try_parse_from(["recvrd", "mcp", "--debug"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Mcp {
                url: None,
                debug: true
            }
        ));
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(Cli::try_parse_from(["recvrd", "wat"]).is_err());
    }

    #[test]
    fn parses_cache_clean() {
        let cli = Cli::try_parse_from(["recvrd", "cache", "clean"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Cache {
                command: CacheCommand::Clean {
                    scope: CliCacheScope::All,
                    url: None
                }
            }
        ));
    }

    #[test]
    fn parses_cache_clean_scope_and_url() {
        let cli = Cli::try_parse_from([
            "recvrd",
            "cache",
            "clean",
            "--scope",
            "downloads",
            "--url",
            "http://127.0.0.1:9000",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Cache {
                command: CacheCommand::Clean {
                    scope: CliCacheScope::Downloads,
                    url: Some(url),
                }
            } if url == "http://127.0.0.1:9000"
        ));
    }

    #[test]
    fn rejects_cache_without_subcommand() {
        assert!(Cli::try_parse_from(["recvrd", "cache"]).is_err());
    }

    #[test]
    fn rejects_unknown_cache_scope() {
        assert!(Cli::try_parse_from(["recvrd", "cache", "clean", "--scope", "music"]).is_err());
    }

    #[test]
    fn cache_scope_maps_to_api_scope() {
        assert_eq!(CliCacheScope::Downloads.api_scope(), CacheScope::Downloads);
        assert_eq!(CliCacheScope::Covers.api_scope(), CacheScope::Covers);
        assert_eq!(CliCacheScope::All.api_scope(), CacheScope::All);
    }

    #[test]
    fn format_bytes_uses_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MiB");
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
