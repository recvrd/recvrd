//! YouTube track download through yt-dlp or the rusty-ytdl backend.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use async_trait::async_trait;
use download_manager::{DownloadError, TrackDownloader};
use recvrd_core::Track;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};

/// Download backend for YouTube tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YoutubeBackend {
    #[default]
    YtDlp,
    RustyYtdl,
}

/// Download YouTube tracks with the selected backend.
pub struct YoutubeDownloader {
    yt_dlp_path: PathBuf,
    backend: YoutubeBackend,
}

impl YoutubeDownloader {
    pub fn new() -> Self {
        Self {
            yt_dlp_path: PathBuf::from("yt-dlp"),
            backend: YoutubeBackend::YtDlp,
        }
    }

    pub fn with_yt_dlp_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.yt_dlp_path = path.into();
        self
    }

    pub fn with_backend(mut self, backend: YoutubeBackend) -> Self {
        self.backend = backend;
        self
    }
}

impl Default for YoutubeDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TrackDownloader for YoutubeDownloader {
    async fn download(
        &self,
        track: &Track,
        output: &Path,
        progress: &(dyn Fn(usize) + Send + Sync),
    ) -> Result<(), DownloadError> {
        match self.backend {
            YoutubeBackend::YtDlp => {
                download_with_ytdlp(&self.yt_dlp_path, &track.track_id, output, progress).await
            }
            YoutubeBackend::RustyYtdl => {
                #[cfg(feature = "rusty-ytdl-backend")]
                {
                    download_with_rusty_ytdl(&track.track_id, output, progress).await
                }
                #[cfg(not(feature = "rusty-ytdl-backend"))]
                {
                    let _ = (track, output, progress);
                    Err(DownloadError::BackendFailed(
                        "rusty-ytdl backend was not compiled".to_string(),
                    ))
                }
            }
        }
    }
}

pub(crate) fn parse_ytdlp_progress(line: &str) -> Option<usize> {
    let value = line
        .trim()
        .strip_prefix("download:")?
        .trim()
        .trim_end_matches('%');
    value
        .parse::<f64>()
        .ok()
        .map(|percent| percent.clamp(0.0, 100.0) as usize)
}

async fn download_with_ytdlp(
    yt_dlp: &Path,
    video_id: &str,
    output_path: &Path,
    progress: &(dyn Fn(usize) + Send + Sync),
) -> Result<(), DownloadError> {
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let output = output_path.to_string_lossy().into_owned();
    progress(0);

    let mut child = Command::new(yt_dlp)
        .args([
            "--no-playlist",
            "-f",
            "bestaudio[ext=m4a]/bestaudio[ext=mp4]/bestaudio",
            "--merge-output-format",
            "mp4",
            "-o",
            &output,
            "--newline",
            "--progress-template",
            "download:%(progress._percent_str)s",
            &url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Dropping this future cancels the download. Kill the child when
        // its handle is dropped, or it keeps running and keeps writing
        // to the part file while the next attempt deletes it.
        .kill_on_drop(true)
        .spawn()?;

    let stdout = child.stdout.take().ok_or_else(|| {
        DownloadError::IoError(std::io::Error::other("yt-dlp stdout was not captured"))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        DownloadError::IoError(std::io::Error::other("yt-dlp stderr was not captured"))
    })?;

    let stderr_task = tokio::spawn(async move {
        let mut buffer = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut buffer).await;
        buffer
    });

    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        if let Some(percent) = parse_ytdlp_progress(&line) {
            progress(percent);
        }
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(DownloadError::BackendFailed(stderr));
    }

    progress(100);
    Ok(())
}

#[cfg(feature = "rusty-ytdl-backend")]
async fn download_with_rusty_ytdl(
    video_id: &str,
    output_path: &Path,
    progress: &(dyn Fn(usize) + Send + Sync),
) -> Result<(), DownloadError> {
    use rusty_ytdl::{DownloadOptions, Video, VideoOptions, VideoQuality, VideoSearchOptions};
    use std::{io::Write, sync::Arc};

    let search_options = VideoSearchOptions::Custom(Arc::new(|format| {
        format.has_audio && !format.has_video && format.mime_type.container == "mp4"
    }));
    let video_options = VideoOptions {
        quality: VideoQuality::Custom(
            search_options.clone(),
            Arc::new(|x, y| x.audio_bitrate.cmp(&y.audio_bitrate)),
        ),
        filter: search_options,
        download_options: DownloadOptions {
            dl_chunk_size: Some(1024 * 100_u64),
        },
        ..Default::default()
    };

    let failed = |message: String| DownloadError::BackendFailed(message);
    let video = Video::new_with_options(video_id, video_options)
        .map_err(|error| failed(error.to_string()))?;
    progress(0);

    let stream = video
        .stream()
        .await
        .map_err(|error| failed(error.to_string()))?;
    let length = stream.content_length();

    let mut file = std::fs::File::create(output_path).map_err(|error| failed(error.to_string()))?;

    let mut total = 0;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|error| failed(error.to_string()))?
    {
        total += chunk.len();
        progress((total as f64 / length as f64 * 100.0) as usize);
        file.write_all(&chunk)
            .map_err(|error| failed(error.to_string()))?;
    }

    file.flush().map_err(|error| failed(error.to_string()))?;

    if total != length || length == 0 {
        std::fs::remove_file(output_path).map_err(|error| failed(error.to_string()))?;
        return Err(failed(format!(
            "Downloaded file is not the same size as the content length ({}/{})",
            total, length
        )));
    }

    progress(100);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use common_structs::ProviderId;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
    };
    use tempfile::tempdir;

    fn track(id: &str) -> Track {
        Track {
            provider: ProviderId::Youtube,
            track_id: id.to_string(),
            title: format!("Track {id}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
        }
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-yt-dlp");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn parses_progress_lines() {
        assert_eq!(parse_ytdlp_progress("download:  0.0%"), Some(0));
        assert_eq!(parse_ytdlp_progress("download: 42.5%"), Some(42));
        assert_eq!(parse_ytdlp_progress("download:100.0%"), Some(100));
        assert_eq!(parse_ytdlp_progress("download: 150.0%"), Some(100));
        assert_eq!(parse_ytdlp_progress("not progress"), None);
        assert_eq!(parse_ytdlp_progress("download: n/a%"), None);
    }

    #[tokio::test]
    async fn downloads_with_a_fake_yt_dlp_script() {
        let dir = tempdir().unwrap();
        let body = "out=\"\"\nwhile [ $# -gt 0 ]; do case \"$1\" in -o) out=\"$2\"; shift 2;; *) shift;; esac; done\necho 'download:  0.0%'\necho 'download: 50.0%'\necho fake-audio > \"$out\"\necho 'download:100.0%'";
        let path = script(dir.path(), body);
        let output = dir.path().join("dQw4w9WgXcQ.part");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let reporter = move |percent: usize| seen_clone.lock().unwrap().push(percent);

        let downloader = YoutubeDownloader::new().with_yt_dlp_path(path);
        downloader
            .download(&track("dQw4w9WgXcQ"), &output, &reporter)
            .await
            .unwrap();

        assert_eq!(fs::read(&output).unwrap(), b"fake-audio\n");
        assert!(seen.lock().unwrap().contains(&50));
        assert_eq!(*seen.lock().unwrap().last().unwrap(), 100);
    }

    #[tokio::test]
    async fn failing_script_reports_a_backend_error() {
        let dir = tempdir().unwrap();
        let path = script(dir.path(), "echo boom >&2\nexit 1");
        let output = dir.path().join("dQw4w9WgXcQ.part");

        let downloader = YoutubeDownloader::new().with_yt_dlp_path(path);
        let error = downloader
            .download(&track("dQw4w9WgXcQ"), &output, &|_| {})
            .await
            .unwrap_err();

        assert!(matches!(error, DownloadError::BackendFailed(_)));
        assert!(
            error.to_string().contains("download backend failed"),
            "{error}"
        );
        assert!(error.to_string().contains("boom"), "{error}");
    }
}
