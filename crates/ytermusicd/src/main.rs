use clap::{Parser, Subcommand};
use common_structs::ProviderId;
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use ytermusicd_core::{
    AuthStatus, DaemonConfig, Downloader as ConfigDownloader, Paths, PlaybackService,
    is_loopback_bind, resolve_auth,
};

mod artwork;
mod audio;
mod downloads;
mod mcp;
mod mpris;
mod music;
mod server;
mod web;

use crate::{
    artwork::ArtworkCache,
    audio::RodioSink,
    downloads::{DownloadControl, DownloadService},
    music::{MusicApi, UnavailableMusicApi, YoutubeMusicApi},
};

#[derive(Debug, Parser)]
#[command(
    name = "ytermusicd",
    version,
    about = "Headless YouTube Music daemon with a web UI, MPRIS and MCP"
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
        #[arg(long, env = "YTERMUSICD_BIND")]
        bind: Option<String>,
        /// Port for the HTTP API
        #[arg(long, env = "YTERMUSICD_PORT")]
        port: Option<u16>,
        /// Path to the headers.txt file (Cookie + User-Agent)
        #[arg(long, env = "YTERMUSICD_HEADERS")]
        headers: Option<PathBuf>,
        /// Path to config.toml
        #[arg(long, env = "YTERMUSICD_CONFIG")]
        config: Option<PathBuf>,
    },
    /// Run the stdio MCP proxy
    Mcp {
        /// Base URL of the running daemon
        #[arg(long, env = "YTERMUSICD_URL")]
        url: Option<String>,
    },
}

fn main() -> ExitCode {
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
                eprintln!("ytermusicd serve failed: {error}");
                ExitCode::FAILURE
            }
        },
        Command::Mcp { url } => match run_mcp(url) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("ytermusicd mcp failed: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run_mcp(url: Option<String>) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(mcp::run(url))
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
    let config = match config {
        Some(path) => DaemonConfig::load(&path),
        None => DaemonConfig::load_or_default(&config_path),
    }
    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()))?;

    let bind = bind
        .or(config.bind)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.or(config.port).unwrap_or(8080);
    let headers = headers.or(config.headers);

    let (auth_status, auth) = match resolve_auth(&paths, headers.as_deref()) {
        Ok(loaded) => {
            log::info!("loaded auth from {}", loaded.source.display());
            if loaded.legacy {
                log::warn!(
                    "legacy headers path in use; move it to {}",
                    paths.headers_file().display()
                );
            }
            (AuthStatus::ok(loaded.source), Some(loaded.auth))
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
        ConfigDownloader::Ytdlp => download_manager::Downloader::YtDlp,
        ConfigDownloader::RustyYtdl => {
            log::warn!("rusty-ytdl backend requested but not compiled; using yt-dlp");
            download_manager::Downloader::YtDlp
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
    log::info!("starting ytermusicd serve on {bind}:{port}");
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
        let music: Arc<dyn MusicApi> = match auth {
            Some(auth) => match auth.create_client().await {
                Ok(client) => {
                    log::info!("YouTube Music client ready");
                    Arc::new(YoutubeMusicApi::new(client))
                }
                Err(error) => {
                    log::error!("could not create YouTube Music client: {error:?}");
                    Arc::new(UnavailableMusicApi)
                }
            },
            None => Arc::new(UnavailableMusicApi),
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

        let downloads = DownloadService::new(cache_dir, parallel_downloads, downloader);
        downloads.spawn(playback.clone());
        let restored_tracks: Vec<ytermusicd_core::Track> = playback
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
        let artwork = Arc::new(ArtworkCache::new(covers_dir, cover_cache_bytes));

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
            commands,
            dbus,
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
        let cli = Cli::try_parse_from(["ytermusicd", "serve"]).unwrap();
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
            "ytermusicd",
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
        let cli = Cli::try_parse_from(["ytermusicd", "mcp"]).unwrap();
        assert!(matches!(cli.command, Command::Mcp { url: None }));
    }

    #[test]
    fn parses_mcp_url() {
        let cli =
            Cli::try_parse_from(["ytermusicd", "mcp", "--url", "http://127.0.0.1:9000"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Mcp { url: Some(url) } if url == "http://127.0.0.1:9000"
        ));
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(Cli::try_parse_from(["ytermusicd", "wat"]).is_err());
    }
}
