//! YouTube Music provider for recvrd: API client, cover fetcher,
//! downloader and the legacy TUI conversion helpers.

mod api;
mod cover;
mod download;

pub use api::{
    AUTH_EXPIRED_MESSAGE, YoutubeMusicApi, connect, playlist_dto, ref_from_track, track_dto,
    track_from_ref,
};
pub use cover::{YoutubeCoverFetcher, cover_urls};
pub use download::{YoutubeBackend, YoutubeDownloader};
pub use ytpapi2::YoutubeMusicError;
