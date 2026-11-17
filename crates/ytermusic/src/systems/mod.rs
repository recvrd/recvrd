use std::sync::Arc;

use download_manager::DownloadManager;
use once_cell::sync::Lazy;
use provider_youtube::{YoutubeBackend, YoutubeDownloader};

use crate::{
    DATABASE,
    config::DownloaderConfig,
    consts::{CACHE_DIR, CONFIG},
};

pub mod logger;
pub mod player;

pub static DOWNLOAD_MANAGER: Lazy<DownloadManager> = Lazy::new(|| {
    let backend = match CONFIG.global.downloader {
        DownloaderConfig::Ytdlp => YoutubeBackend::YtDlp,
        #[cfg(feature = "rusty-ytdl-backend")]
        DownloaderConfig::RustyYtdl => YoutubeBackend::RustyYtdl,
        #[cfg(not(feature = "rusty-ytdl-backend"))]
        DownloaderConfig::RustyYtdl => {
            log::warn!("rusty-ytdl-backend not compiled, using yt-dlp");
            YoutubeBackend::YtDlp
        }
    };
    DownloadManager::new(
        CACHE_DIR.to_path_buf(),
        &DATABASE,
        CONFIG.global.parallel_downloads,
        Arc::new(YoutubeDownloader::new().with_backend(backend)),
    )
});
