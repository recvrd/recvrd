use std::{
    path::Path,
    string::FromUtf8Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::path::PathBuf;

use json_extractor::{
    Continuation, extract_playlist_info, from_json, get_continuation, get_playlist,
    get_playlist_search, get_video, get_video_from_album,
};
use log::{debug, error, trace};
pub use reqwest::header::HeaderMap;
pub use reqwest::header::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use string_utils::StringUtils;

mod json_extractor;
mod string_utils;

pub use json_extractor::YoutubeMusicVideoRef;

pub type Result<T> = std::result::Result<T, YoutubeMusicError>;

const YTM_DOMAIN: &str = "https://music.youtube.com";

/// Bound on one YouTube API call. A stalled response must not hang
/// the API handler that waits on it.
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Retries for a rate limit or server error on a browse call.
const MAX_BROWSE_RETRIES: u32 = 2;

#[cfg(test)]
fn headers_path() -> PathBuf {
    std::env::var_os("YTERMUSICD_HEADERS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../headers.txt"))
}

#[cfg(test)]
fn get_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    let path = headers_path();
    let file = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "failed to read headers from {}: {e}\n\
             set YTERMUSICD_HEADERS or place headers.txt at the repository root",
            path.display()
        )
    });
    for header in file.lines() {
        if header.trim().is_empty() {
            continue;
        }
        let (key, value) = header.split_once(": ").unwrap();
        headers.insert(
            match key {
                "Cookie" => reqwest::header::COOKIE,
                "User-Agent" => reqwest::header::USER_AGENT,
                _ => {
                    println!("Unknown header key: {}", key);
                    continue;
                }
            },
            value.parse().unwrap(),
        );
    }
    headers
}

#[cfg(test)]
fn get_account_id() -> Option<String> {
    let path = headers_path().parent()?.join("account_id.txt");
    std::fs::read_to_string(path)
        .ok()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

#[test]
#[ignore = "requires headers.txt and network access"]
fn advanced_like() {
    use tokio::runtime::Runtime;
    Runtime::new().unwrap().block_on(async {
        let ytm = YoutubeMusicInstance::new(get_headers(), get_account_id())
            .await
            .unwrap();
        println!("{}", ytm.compute_sapi_hash());
        let search = ytm
            .get_library(&Endpoint::MusicLibraryLanding, 0)
            .await
            .unwrap();
        assert!(!search.is_empty());
        println!("{:?}", search[1]);
        println!("{:?}", ytm.get_playlist(&search[1], 0).await.unwrap());
    });
}

#[test]
#[ignore = "requires headers.txt and network access"]
fn advanced_test() {
    use tokio::runtime::Runtime;
    Runtime::new().unwrap().block_on(async {
        let ytm = YoutubeMusicInstance::new(get_headers(), get_account_id())
            .await
            .unwrap();
        let search = ytm.search("j'ai la danse qui va avec", 0).await.unwrap();
        assert!(!search.videos.is_empty());
        assert!(!search.playlists.is_empty());
        let playlist_contents = ytm.get_playlist(&search.playlists[1], 0).await.unwrap();
        println!("{:?}", playlist_contents);
    });
}

#[test]
#[ignore = "requires headers.txt and network access"]
fn home_test() {
    use tokio::runtime::Runtime;
    Runtime::new().unwrap().block_on(async {
        let ytm = YoutubeMusicInstance::new(get_headers(), get_account_id())
            .await
            .unwrap();
        let search = ytm.get_home(0).await.unwrap();
        println!("{:?}", search.playlists);
        assert!(!search.playlists.is_empty());
        let playlist_contents = ytm.get_playlist(&search.playlists[0], 0).await.unwrap();
        println!("{:?}", playlist_contents);
    });
}

#[derive(Debug, Clone, PartialOrd, Eq, Ord, PartialEq, Hash, Serialize, Deserialize)]
pub struct YoutubeMusicPlaylistRef {
    pub name: String,
    pub subtitle: String,
    pub browse_id: String,
}

pub struct YoutubeMusicInstance {
    sapisid: String,
    innertube_api_key: String,
    client_version: String,
    cookies: String,
    account_id: Option<String>,
    client: reqwest::Client,
    base_url: String,
}

/// Consume one continuation from the budget and report whether the response
/// should request another page.
fn consume_continuation(remaining: &mut usize) -> bool {
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    *remaining > 0
}

impl YoutubeMusicInstance {
    pub async fn from_header_file(path: &Path) -> Result<Self> {
        let mut headers = HeaderMap::new();
        for header in tokio::fs::read_to_string(path)
            .await
            .map_err(YoutubeMusicError::IoError)?
            .lines()
        {
            if let Some((key, value)) = header.split_once(": ") {
                headers.insert(
                    match key.to_lowercase().as_str() {
                        "cookie" => reqwest::header::COOKIE,
                        "user-agent" => reqwest::header::USER_AGENT,
                        _ => {
                            #[cfg(test)]
                            println!("Unknown header key: {key}");
                            continue;
                        }
                    },
                    value.parse().unwrap(),
                );
            }
        }
        if !headers.contains_key(reqwest::header::COOKIE) {
            return Err(YoutubeMusicError::InvalidHeaders);
        }
        if !headers.contains_key(reqwest::header::USER_AGENT) {
            headers.insert(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64; rv:108.0) Gecko/20100101 Firefox/108.0"
                    .parse()
                    .unwrap(),
            );
        }
        let account_path = path
            .parent()
            .unwrap_or(Path::new("../"))
            .join("account_id.txt");
        let account_id = match tokio::fs::read_to_string(account_path).await {
            Ok(mut id) => {
                if id.ends_with("\n") {
                    id.pop();
                    if id.ends_with("\r") {
                        id.pop();
                    }
                }
                Some(id)
            }
            Err(_) => None, //don't care if there is no files or nothing in the file
        };
        Self::new(headers, account_id).await
    }

    pub async fn new(headers: HeaderMap, account_id: Option<String>) -> Result<Self> {
        trace!("Creating new YoutubeMusicInstance");
        let rest_client = reqwest::ClientBuilder::default()
            .default_headers(headers.clone())
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(YoutubeMusicError::RequestError)?;
        trace!("Fetching YoutubeMusic homepage");
        let response: String = rest_client
            .get(YTM_DOMAIN)
            .headers(headers.clone())
            .send()
            .await
            .map_err(YoutubeMusicError::RequestError)?
            .text()
            .await
            .map_err(YoutubeMusicError::RequestError)?;
        trace!("Fetched");

        if response.contains("<base href=\"https://accounts.google.com/v3/signin/\">")
            || response.contains("<base href=\"https://consent.youtube.com/\">")
        {
            error!("Need to login");
            return Err(YoutubeMusicError::NeedToLogin);
        }
        trace!("Parsing cookies");
        let cookies = headers
            .get("Cookie")
            .ok_or(YoutubeMusicError::NoCookieAttribute)?;
        let cookies_bytes = cookies.as_bytes();
        let cookies = String::from_utf8(cookies_bytes.to_vec())
            .map_err(YoutubeMusicError::InvalidCookie)?
            .to_string();
        let sapisid = cookies
            .between("SAPISID=", ";")
            .ok_or_else(|| YoutubeMusicError::NoSapsidInCookie)?;
        trace!("Cookies parsed (SAPISID redacted)");
        let innertube_api_key = response
            .between("INNERTUBE_API_KEY\":\"", "\"")
            .ok_or_else(|| YoutubeMusicError::CantFindInnerTubeApiKey(response.to_string()))?;
        trace!("Innertube API key: {}", innertube_api_key);
        let client_version = response
            .between("INNERTUBE_CLIENT_VERSION\":\"", "\"")
            .ok_or_else(|| {
                YoutubeMusicError::CantFindInnerTubeClientVersion(response.to_string())
            })?;
        trace!("Innertube client version: {}", client_version);
        // New file for brand accounts, maybe put it in config or headers.txt is better but more complex.
        trace!("account id {:?}", account_id);
        Ok(Self {
            sapisid: sapisid.to_string(),
            innertube_api_key: innertube_api_key.to_string(),
            client_version: client_version.to_string(),
            cookies,
            account_id,
            client: rest_client,
            base_url: YTM_DOMAIN.to_string(),
        })
    }

    #[cfg(test)]
    fn for_test(
        base_url: String,
        innertube_api_key: String,
        client_version: String,
        cookies: String,
        account_id: Option<String>,
    ) -> Self {
        Self {
            sapisid: "test-sapisid".to_string(),
            innertube_api_key,
            client_version,
            cookies,
            account_id,
            client: reqwest::Client::new(),
            base_url,
        }
    }
    fn compute_sapi_hash(&self) -> String {
        let start = SystemTime::now();
        let since_the_epoch = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let timestamp = since_the_epoch.as_secs();
        let mut hasher = Sha1::new();
        hasher.update(format!("{timestamp} {} {YTM_DOMAIN}", self.sapisid));
        let result = hasher.finalize();
        let mut hex = String::with_capacity(40);
        for byte in result {
            hex.push_str(&format!("{byte:02x}"));
        }
        trace!("Computed SAPI hash (redacted)");
        format!("{timestamp}_{hex}")
    }
    async fn browse_continuation(
        &self,
        continuation: &Continuation,
        continuations: bool,
    ) -> Result<(Value, Vec<Continuation>)> {
        let playlist_json: Value =
            serde_json::from_str(&self.browse_continuation_raw(continuation).await?)
                .map_err(YoutubeMusicError::SerdeJson)?;
        debug!("Browse continuation response: {playlist_json}");
        if playlist_json.get("error").is_some() {
            error!("Error in browse_continuation");
            error!("{:?}", playlist_json);
            return Err(YoutubeMusicError::YoutubeMusicError(playlist_json));
        }
        let continuation = if continuations {
            from_json(&playlist_json, get_continuation)?
        } else {
            Vec::new()
        };
        Ok((playlist_json, continuation))
    }
    /// Send a browse request, with a bounded retry for rate limits and
    /// server errors. Browse calls only read state, so a retry is safe.
    /// A transport error returns at once. The delay grows 500 ms, then 1 s.
    async fn send_browse_with_retry(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        let mut attempt = 0;
        loop {
            let response = build()
                .send()
                .await
                .map_err(YoutubeMusicError::RequestError)?;
            let status = response.status();
            if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                && attempt < MAX_BROWSE_RETRIES
            {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                continue;
            }
            return Ok(response);
        }
    }

    async fn browse_continuation_raw(
        &self,
        Continuation {
            continuation,
            click_tracking_params,
        }: &Continuation,
    ) -> Result<String> {
        trace!("Browse continuation {continuation}");
        let url = innertube_url(
            "browse",
            &[
                ("ctoken", continuation),
                ("continuation", continuation),
                ("type", "next"),
                ("itct", click_tracking_params),
                ("key", &self.innertube_api_key),
                ("prettyPrint", "false"),
            ],
        )?;
        let body = json!({ "context": self.context() });
        self.send_browse_with_retry(|| {
            self.client
                .post(url.clone())
                .header("Content-Type", "application/json")
                .header(
                    "Authorization",
                    format!("SAPISIDHASH {}", self.compute_sapi_hash()),
                )
                .header("X-Origin", "https://music.youtube.com")
                .header("Cookie", &self.cookies)
                .body(body.to_string())
        })
        .await?
        .text()
        .await
        .map_err(YoutubeMusicError::RequestError)
    }
    async fn browse_raw(
        &self,
        endpoint_route: &str,
        endpoint_key: &str,
        endpoint_param: &str,
    ) -> Result<String> {
        trace!("Browse {endpoint_route}");
        let url = innertube_url(
            endpoint_route,
            &[("key", &self.innertube_api_key), ("prettyPrint", "false")],
        )?;
        let body = self.browse_body(endpoint_key, endpoint_param);
        self.send_browse_with_retry(|| {
            self.client
                .post(url.clone())
                .header("Content-Type", "application/json")
                .header(
                    "Authorization",
                    format!("SAPISIDHASH {}", self.compute_sapi_hash()),
                )
                .header("X-Origin", "https://music.youtube.com")
                .header("Cookie", &self.cookies)
                .body(body.to_string())
        })
        .await?
        .text()
        .await
        .map_err(YoutubeMusicError::RequestError)
    }
    async fn browse(
        &self,
        endpoint: &Endpoint,
        continuations: bool,
    ) -> Result<(serde_json::Value, Vec<Continuation>)> {
        let playlist_json: Value = serde_json::from_str(
            &self
                .browse_raw(
                    &endpoint.get_route(),
                    &endpoint.get_key(),
                    &endpoint.get_param(),
                )
                .await?,
        )
        .map_err(YoutubeMusicError::SerdeJson)?;
        debug!("Browse response: {playlist_json}");
        if playlist_json.get("error").is_some() {
            error!("Error in browse ({endpoint:?})");
            error!("{:?}", playlist_json);
            return Err(YoutubeMusicError::YoutubeMusicError(playlist_json));
        }
        let continuation = if continuations {
            from_json(&playlist_json, get_continuation)?
        } else {
            Vec::new()
        };
        Ok((playlist_json, continuation))
    }
    pub async fn get_library(
        &self,
        endpoint: &Endpoint,
        mut n_continuations: usize,
    ) -> Result<Vec<YoutubeMusicPlaylistRef>> {
        let (library_json, mut continuations) = self.browse(endpoint, n_continuations > 0).await?;
        trace!("Fetched library");
        debug!("Library response: {library_json}");
        debug!("Continuations: {continuations:?}");
        let mut library = from_json(&library_json, get_playlist)?;
        debug!("Library: {library:?}");
        while let Some(continuation) = continuations.pop() {
            let fetch_more = consume_continuation(&mut n_continuations);
            trace!("Fetching continuation {continuation:?} ({endpoint:?})");
            let (library_json, new_continuations) =
                self.browse_continuation(&continuation, fetch_more).await?;
            debug!("Library response: {library_json}");
            continuations.extend(new_continuations);
            let new_library = from_json(&library_json, get_playlist)?;
            trace!("Fetched {} playlists", new_library.len());
            debug!("Library response: {library_json}");
            library.extend(new_library);
            if n_continuations == 0 {
                break;
            }
        }

        Ok(library)
    }
    pub async fn get_playlist(
        &self,
        playlist: &YoutubeMusicPlaylistRef,
        n_continuations: usize,
    ) -> Result<Vec<YoutubeMusicVideoRef>> {
        self.get_playlist_raw(&playlist.browse_id, n_continuations)
            .await
    }
    pub async fn get_playlist_raw(
        &self,
        playlist_id: &str,
        mut n_continuations: usize,
    ) -> Result<Vec<YoutubeMusicVideoRef>> {
        let (playlist_json, mut continuations) = self
            .browse(
                &Endpoint::Playlist(playlist_id.to_string()),
                n_continuations > 0,
            )
            .await?;
        trace!("Fetched playlist");
        debug!("Playlist response: {playlist_json}");
        debug!("Continuations: {continuations:?}");
        let mut videos = parse_playlist(&playlist_json)?;

        debug!("Videos: {videos:?}");

        while let Some(continuation) = continuations.pop() {
            let fetch_more = consume_continuation(&mut n_continuations);
            trace!("Fetching continuation {continuation:?}");
            let (playlist_json, new_continuations) =
                self.browse_continuation(&continuation, fetch_more).await?;
            debug!("Playlist response: {playlist_json}");
            continuations.extend(new_continuations);
            let new_videos = parse_playlist(&playlist_json)?;
            trace!("Fetched {} videos", new_videos.len());
            debug!("Playlist response: {playlist_json}");
            videos.extend(new_videos);
            if n_continuations == 0 {
                break;
            }
        }

        Ok(videos)
    }
    pub async fn search(
        &self,
        search_query: &str,
        mut n_continuations: usize,
    ) -> Result<SearchResults> {
        let (search_json, mut continuations) = self
            .browse(&Endpoint::Search(search_query.to_string()), false)
            .await?;
        debug!("Search response: {search_json}");
        let mut videos = from_json(&search_json, get_video)?;
        debug!("Videos: {videos:?}");
        let mut playlists = from_json(&search_json, get_playlist_search)?;
        debug!("Playlists: {playlists:?}");

        while let Some(continuation) = continuations.pop() {
            let fetch_more = consume_continuation(&mut n_continuations);
            trace!("Fetching continuation {continuation:?}");
            let (search_json, new_continuations) =
                self.browse_continuation(&continuation, fetch_more).await?;
            trace!("Search response: {search_json}");
            continuations.extend(new_continuations);
            let new_videos = from_json(&search_json, get_video)?;
            debug!("Videos: {videos:?}");
            let new_playlists = from_json(&search_json, get_playlist_search)?;
            debug!("Playlists: {playlists:?}");
            videos.extend(new_videos);
            playlists.extend(new_playlists);
            if n_continuations == 0 {
                break;
            }
        }

        Ok(SearchResults { videos, playlists })
    }

    pub async fn get_home(&self, mut n_continuations: usize) -> Result<SearchResults> {
        let (home_json, mut continuations) = self
            .browse(&Endpoint::MusicHome, n_continuations > 0)
            .await?;
        debug!("Home response: {home_json}");
        let mut videos = from_json(&home_json, get_video)?;
        debug!("Videos: {videos:?}");
        let mut playlists = from_json(&home_json, get_playlist)?;
        debug!("Playlists: {playlists:?}");

        while let Some(continuation) = continuations.pop() {
            let fetch_more = consume_continuation(&mut n_continuations);
            trace!("Fetching continuation {continuation:?}");
            let (home_json, new_continuations) =
                self.browse_continuation(&continuation, fetch_more).await?;
            debug!("Home response: {home_json}");
            continuations.extend(new_continuations);
            let new_videos = from_json(&home_json, get_video)?;
            debug!("Videos: {videos:?}");
            let new_playlists = from_json(&home_json, get_playlist)?;
            debug!("Playlists: {playlists:?}");
            videos.extend(new_videos);
            playlists.extend(new_playlists);
            if n_continuations == 0 {
                break;
            }
        }
        Ok(SearchResults { playlists, videos })
    }

    /// Build a browse request body with `serde_json`, so a search query
    /// or playlist ID with quotes or backslashes cannot break out of
    /// the JSON string.
    fn browse_body(&self, endpoint_key: &str, endpoint_param: &str) -> Value {
        let mut body = json!({});
        body["context"] = self.context();
        body[endpoint_key] = Value::String(endpoint_param.to_string());
        body
    }

    fn context(&self) -> Value {
        let client = json!({
            "clientName": "WEB_REMIX",
            "clientVersion": self.client_version,
        });
        match &self.account_id {
            Some(id) => json!({
                "client": client,
                "user": { "onBehalfOfUser": id },
            }),
            None => json!({ "client": client }),
        }
    }

    async fn post_edit(&self, route: &str, mut body: Value) -> Result<Value> {
        body["context"] = self.context();
        let url = format!(
            "{}/youtubei/v1/{route}?key={}&prettyPrint=false",
            self.base_url, self.innertube_api_key
        );
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header(
                "Authorization",
                format!("SAPISIDHASH {}", self.compute_sapi_hash()),
            )
            .header("X-Origin", self.base_url.as_str())
            .header("Cookie", &self.cookies)
            .body(body.to_string())
            .send()
            .await
            .map_err(YoutubeMusicError::RequestError)?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(YoutubeMusicError::RequestError)?;
        let response: Value = serde_json::from_str(&text).map_err(YoutubeMusicError::SerdeJson)?;
        classify_edit_response(status, response)
    }

    /// Create a playlist and return its browse ID.
    pub async fn create_playlist(
        &self,
        title: &str,
        description: &str,
        privacy: PlaylistPrivacy,
    ) -> Result<String> {
        let body = create_playlist_body(title, description, privacy);
        let response = self.post_edit("playlist/create", body).await?;
        let id = response
            .get("playlistId")
            .and_then(Value::as_str)
            .ok_or_else(|| YoutubeMusicError::YoutubeMusicError(response.clone()))?;
        if id.starts_with("VL") {
            Ok(id.to_string())
        } else {
            Ok(format!("VL{id}"))
        }
    }

    /// Rename a playlist.
    pub async fn rename_playlist(&self, playlist_id: &str, title: &str) -> Result<()> {
        let actions = vec![json!({
            "action": "ACTION_SET_PLAYLIST_NAME",
            "playlistName": title,
        })];
        let body = edit_playlist_body(playlist_id, actions)?;
        let response = self.post_edit("browse/edit_playlist", body).await?;
        ensure_edit_succeeded(&response)
    }

    /// Delete a playlist.
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<()> {
        let body = json!({ "playlistId": edit_playlist_id(playlist_id)? });
        let response = self.post_edit("playlist/delete", body).await?;
        ensure_edit_succeeded(&response)
    }

    /// Add tracks to a playlist.
    pub async fn add_playlist_items(&self, playlist_id: &str, video_ids: &[String]) -> Result<()> {
        if video_ids.is_empty() {
            return Err(YoutubeMusicError::Other("no video IDs".to_string()));
        }
        let body = edit_playlist_body(playlist_id, add_video_actions(video_ids))?;
        let response = self.post_edit("browse/edit_playlist", body).await?;
        ensure_edit_succeeded(&response)
    }

    /// Remove playlist items. Each item needs its video ID and set video ID.
    pub async fn remove_playlist_items(
        &self,
        playlist_id: &str,
        items: &[(String, String)],
    ) -> Result<()> {
        if items.is_empty() {
            return Err(YoutubeMusicError::Other("no playlist items".to_string()));
        }
        let body = edit_playlist_body(playlist_id, remove_video_actions(items))?;
        let response = self.post_edit("browse/edit_playlist", body).await?;
        ensure_edit_succeeded(&response)
    }

    /// Move one playlist item before a successor item. A missing successor
    /// moves the item to the end.
    pub async fn move_playlist_item(
        &self,
        playlist_id: &str,
        set_video_id: &str,
        successor: Option<&str>,
    ) -> Result<()> {
        let actions = vec![move_video_action(set_video_id, successor)];
        let body = edit_playlist_body(playlist_id, actions)?;
        let response = self.post_edit("browse/edit_playlist", body).await?;
        ensure_edit_succeeded(&response)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistPrivacy {
    Public,
    Private,
    Unlisted,
}

impl PlaylistPrivacy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "PUBLIC",
            Self::Private => "PRIVATE",
            Self::Unlisted => "UNLISTED",
        }
    }
}

/// Build an InnerTube request URL with percent-encoded query pairs,
/// so a continuation token or API key cannot change the query string.
fn innertube_url(route: &str, query: &[(&str, &str)]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("{YTM_DOMAIN}/youtubei/v1/{route}"))
        .map_err(|error| YoutubeMusicError::Other(error.to_string()))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url)
}

fn edit_playlist_id(playlist_id: &str) -> Result<&str> {
    let id = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
    if id.is_empty() {
        return Err(YoutubeMusicError::Other("playlist ID is empty".to_string()));
    }
    Ok(id)
}

fn create_playlist_body(title: &str, description: &str, privacy: PlaylistPrivacy) -> Value {
    json!({
        "title": title,
        "description": description,
        "privacyStatus": privacy.as_str(),
    })
}

fn edit_playlist_body(playlist_id: &str, actions: Vec<Value>) -> Result<Value> {
    Ok(json!({
        "playlistId": edit_playlist_id(playlist_id)?,
        "actions": actions,
    }))
}

fn add_video_actions(video_ids: &[String]) -> Vec<Value> {
    video_ids
        .iter()
        .map(|video_id| {
            json!({
                "action": "ACTION_ADD_VIDEO",
                "addedVideoId": video_id,
            })
        })
        .collect()
}

fn remove_video_actions(items: &[(String, String)]) -> Vec<Value> {
    items
        .iter()
        .map(|(video_id, set_video_id)| {
            json!({
                "action": "ACTION_REMOVE_VIDEO",
                "removedVideoId": video_id,
                "setVideoId": set_video_id,
            })
        })
        .collect()
}

fn move_video_action(set_video_id: &str, successor: Option<&str>) -> Value {
    let mut action = json!({
        "action": "ACTION_MOVE_VIDEO_BEFORE",
        "setVideoId": set_video_id,
    });
    if let Some(successor) = successor {
        action["movedSetVideoIdSuccessor"] = Value::String(successor.to_string());
    }
    action
}

fn classify_edit_response(status: reqwest::StatusCode, response: Value) -> Result<Value> {
    let code = response.pointer("/error/code").and_then(Value::as_i64);
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
        || matches!(code, Some(401 | 403))
    {
        return Err(YoutubeMusicError::AuthExpired(response));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || code == Some(429) {
        return Err(YoutubeMusicError::RateLimited(response));
    }
    if !status.is_success() || response.get("error").is_some() {
        return Err(YoutubeMusicError::YoutubeMusicError(response));
    }
    Ok(response)
}

fn ensure_edit_succeeded(response: &Value) -> Result<()> {
    if let Some(status) = response.get("status").and_then(Value::as_str)
        && status != "STATUS_SUCCEEDED"
    {
        return Err(YoutubeMusicError::YoutubeMusicError(response.clone()));
    }
    Ok(())
}

fn parse_playlist(playlist_json: &Value) -> Result<Vec<YoutubeMusicVideoRef>> {
    let mut videos = from_json(playlist_json, get_video)?;
    let info = extract_playlist_info(playlist_json);
    for mut video in from_json(playlist_json, get_video_from_album)? {
        if videos.iter().any(|x| x.video_id == video.video_id) {
            continue;
        }
        if let Some((title, artist)) = info.as_ref() {
            if video.album.is_empty() {
                video.album = title.to_string();
            }
            if video.author.is_empty() {
                video.author = artist.to_string();
            }
        }
        videos.push(video);
    }
    Ok(videos)
}

#[derive(Debug, Clone, PartialOrd, Eq, Ord, PartialEq, Hash)]
pub struct SearchResults {
    pub videos: Vec<YoutubeMusicVideoRef>,
    pub playlists: Vec<YoutubeMusicPlaylistRef>,
}

#[derive(Debug, Clone, PartialOrd, Eq, Ord, PartialEq, Hash)]
pub enum Endpoint {
    MusicLikedPlaylists,
    MusicHome,
    MusicLibraryLanding,
    Playlist(String),
    Search(String),
}

impl Endpoint {
    fn get_key(&self) -> String {
        match self {
            Endpoint::MusicLikedPlaylists => "browseId".to_owned(),
            Endpoint::MusicLibraryLanding => "browseId".to_owned(),
            Endpoint::Playlist(_) => "browseId".to_owned(),
            Endpoint::MusicHome => "browseId".to_owned(),
            Endpoint::Search(_) => "query".to_owned(),
        }
    }
    fn get_param(&self) -> String {
        match self {
            Endpoint::MusicLikedPlaylists => "FEmusic_liked_playlists".to_owned(),
            Endpoint::MusicLibraryLanding => "FEmusic_library_landing".to_owned(),
            Endpoint::Playlist(id) => id.to_owned(),
            Endpoint::Search(query) => query.to_owned(),
            Endpoint::MusicHome => "FEmusic_home".to_owned(),
        }
    }
    fn get_route(&self) -> String {
        match self {
            Endpoint::MusicLikedPlaylists => "browse".to_owned(),
            Endpoint::MusicLibraryLanding => "browse".to_owned(),
            Endpoint::Playlist(_) => "browse".to_owned(),
            Endpoint::Search(_) => "search".to_owned(),
            Endpoint::MusicHome => "browse".to_owned(),
        }
    }
}

#[derive(Debug)]
pub enum YoutubeMusicError {
    RequestError(reqwest::Error),
    Other(String),
    NoCookieAttribute,
    NoSapsidInCookie,
    InvalidCookie(FromUtf8Error),
    NeedToLogin,
    CantFindInnerTubeApiKey(String),
    CantFindInnerTubeClientVersion(String),
    CantFindVisitorData(String),
    SerdeJson(serde_json::Error),
    IoError(std::io::Error),
    YoutubeMusicError(Value),
    AuthExpired(Value),
    RateLimited(Value),
    InvalidHeaders,
}

#[cfg(test)]
mod edit_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn normalizes_playlist_ids() {
        assert_eq!(edit_playlist_id("VLPL123").unwrap(), "PL123");
        assert_eq!(edit_playlist_id("PL123").unwrap(), "PL123");
        assert!(edit_playlist_id("VL").is_err());
        assert!(edit_playlist_id("").is_err());
    }

    #[test]
    fn builds_create_body() {
        let body = create_playlist_body("Mix", "A description", PlaylistPrivacy::Unlisted);
        assert_eq!(body["title"], "Mix");
        assert_eq!(body["description"], "A description");
        assert_eq!(body["privacyStatus"], "UNLISTED");
    }

    #[test]
    fn builds_add_actions() {
        let actions = add_video_actions(&["v1".to_string(), "v2".to_string()]);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0]["action"], "ACTION_ADD_VIDEO");
        assert_eq!(actions[0]["addedVideoId"], "v1");
        assert_eq!(actions[1]["addedVideoId"], "v2");
    }

    #[test]
    fn builds_remove_actions() {
        let actions = remove_video_actions(&[("v1".to_string(), "s1".to_string())]);
        assert_eq!(actions[0]["action"], "ACTION_REMOVE_VIDEO");
        assert_eq!(actions[0]["removedVideoId"], "v1");
        assert_eq!(actions[0]["setVideoId"], "s1");
    }

    #[test]
    fn builds_move_actions() {
        let before = move_video_action("s2", Some("s3"));
        assert_eq!(before["action"], "ACTION_MOVE_VIDEO_BEFORE");
        assert_eq!(before["setVideoId"], "s2");
        assert_eq!(before["movedSetVideoIdSuccessor"], "s3");

        let last = move_video_action("s2", None);
        assert!(last.get("movedSetVideoIdSuccessor").is_none());
    }

    #[test]
    fn edit_body_strips_the_browse_prefix() {
        let body = edit_playlist_body(
            "VLPL1",
            vec![json!({
                "action": "ACTION_SET_PLAYLIST_NAME",
                "playlistName": "New name",
            })],
        )
        .unwrap();
        assert_eq!(body["playlistId"], "PL1");
        assert_eq!(body["actions"][0]["playlistName"], "New name");
    }

    #[test]
    fn classifies_auth_and_rate_limit_errors() {
        let auth_body = json!({"error": {"code": 401, "message": "unauthorized"}});
        assert!(matches!(
            classify_edit_response(reqwest::StatusCode::UNAUTHORIZED, auth_body.clone()),
            Err(YoutubeMusicError::AuthExpired(_))
        ));
        assert!(matches!(
            classify_edit_response(reqwest::StatusCode::FORBIDDEN, auth_body),
            Err(YoutubeMusicError::AuthExpired(_))
        ));

        let rate_body = json!({"error": {"code": 429, "message": "quota"}});
        assert!(matches!(
            classify_edit_response(reqwest::StatusCode::TOO_MANY_REQUESTS, rate_body),
            Err(YoutubeMusicError::RateLimited(_))
        ));

        let bad_body = json!({"error": {"code": 400, "message": "bad request"}});
        assert!(matches!(
            classify_edit_response(reqwest::StatusCode::BAD_REQUEST, bad_body),
            Err(YoutubeMusicError::YoutubeMusicError(_))
        ));

        let auth_json = json!({"error": {"code": 401, "message": "unauthorized"}});
        assert!(matches!(
            classify_edit_response(reqwest::StatusCode::OK, auth_json),
            Err(YoutubeMusicError::AuthExpired(_))
        ));
        let rate_json = json!({"error": {"code": 429, "message": "quota"}});
        assert!(matches!(
            classify_edit_response(reqwest::StatusCode::OK, rate_json),
            Err(YoutubeMusicError::RateLimited(_))
        ));
    }

    #[test]
    fn accepts_successful_edits() {
        let response = json!({"status": "STATUS_SUCCEEDED"});
        assert!(classify_edit_response(reqwest::StatusCode::OK, response.clone()).is_ok());
        assert!(ensure_edit_succeeded(&response).is_ok());
        assert!(ensure_edit_succeeded(&json!({})).is_ok());
        assert!(ensure_edit_succeeded(&json!({"status": "STATUS_FAILED"})).is_err());
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;

        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let count = socket.read(&mut chunk).await.unwrap();
            if count == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..count]);
            if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = position + 4;
                let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if buffer.len() >= header_end + length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buffer).to_string()
    }

    #[tokio::test]
    async fn posts_playlist_edits_with_auth_and_context() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let _ = request_tx.send(request);
            let response = r#"{"playlistId":"PL123"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
        });

        let instance = YoutubeMusicInstance::for_test(
            format!("http://{address}"),
            "test-key".to_string(),
            "1.0".to_string(),
            "SAPISID=x".to_string(),
            None,
        );
        let id = instance
            .create_playlist("Mix", "desc", PlaylistPrivacy::Unlisted)
            .await
            .unwrap();
        assert_eq!(id, "VLPL123");

        let request = request_rx.await.unwrap();
        let request_lower = request.to_lowercase();
        assert!(request.starts_with("POST /youtubei/v1/playlist/create?key=test-key"));
        assert!(request_lower.contains("authorization: sapisidhash "));
        assert!(request_lower.contains(&format!("x-origin: http://{address}")));

        let body = request.split("\r\n\r\n").nth(1).unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["title"], "Mix");
        assert_eq!(body["privacyStatus"], "UNLISTED");
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
    }

    /// Serve a fixed sequence of statuses, one per connection, and
    /// count the requests. Each connection closes after its reply.
    async fn serve_statuses(
        listener: tokio::net::TcpListener,
        statuses: Vec<reqwest::StatusCode>,
        hits: Arc<AtomicUsize>,
    ) {
        use tokio::io::AsyncWriteExt;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let index = hits.fetch_add(1, Ordering::SeqCst);
            let Some(status) = statuses.get(index) else {
                return;
            };
            let _ = read_request(&mut socket).await;
            let body = "{}";
            let reply = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                body.len(),
                body
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    }

    /// Drive one browse attempt against a local status sequence and
    /// report the final status and the request count.
    async fn browse_attempts(statuses: Vec<reqwest::StatusCode>) -> (reqwest::StatusCode, usize) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let server = tokio::spawn(serve_statuses(listener, statuses, hits.clone()));
        let instance = YoutubeMusicInstance::for_test(
            format!("http://{address}"),
            "test-key".to_string(),
            "1.0".to_string(),
            "SAPISID=x".to_string(),
            None,
        );
        let response = instance
            .send_browse_with_retry(|| {
                instance
                    .client
                    .post(format!("http://{address}/youtubei/v1/browse"))
                    .header("Content-Type", "application/json")
                    .body("{}")
            })
            .await
            .unwrap();
        let status = response.status();
        server.abort();
        (status, hits.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn browse_retries_server_errors_then_succeeds() {
        let (status, hits) = browse_attempts(vec![
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            reqwest::StatusCode::OK,
        ])
        .await;
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(hits, 3);
    }

    #[tokio::test]
    async fn browse_retries_rate_limits() {
        let (status, hits) = browse_attempts(vec![
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::OK,
        ])
        .await;
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(hits, 2);
    }

    #[tokio::test]
    async fn browse_gives_up_after_max_retries() {
        let (status, hits) =
            browse_attempts(vec![reqwest::StatusCode::INTERNAL_SERVER_ERROR; 3]).await;
        assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(hits, 3);
    }

    #[tokio::test]
    async fn browse_does_not_retry_success_or_client_errors() {
        let (status, hits) = browse_attempts(vec![reqwest::StatusCode::OK]).await;
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(hits, 1);

        let (status, hits) = browse_attempts(vec![reqwest::StatusCode::BAD_REQUEST]).await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(hits, 1);
    }
}

#[cfg(test)]
mod continuation_tests {
    use super::consume_continuation;

    #[test]
    fn zero_budget_does_not_underflow() {
        let mut remaining = 0;
        assert!(!consume_continuation(&mut remaining));
        assert_eq!(remaining, 0);
    }

    #[test]
    fn one_budget_does_not_underflow() {
        let mut remaining = 1;
        assert!(!consume_continuation(&mut remaining));
        assert_eq!(remaining, 0);
    }

    #[test]
    fn many_budget_stops_at_zero() {
        let mut remaining = 3;
        assert!(consume_continuation(&mut remaining));
        assert!(consume_continuation(&mut remaining));
        assert!(!consume_continuation(&mut remaining));
        assert_eq!(remaining, 0);
        assert!(!consume_continuation(&mut remaining));
    }
}

#[cfg(test)]
mod browse_tests {
    use super::*;

    fn instance(account_id: Option<String>) -> YoutubeMusicInstance {
        YoutubeMusicInstance::for_test(
            "http://127.0.0.1:9".to_string(),
            "test-key".to_string(),
            "1.0".to_string(),
            "SAPISID=x".to_string(),
            account_id,
        )
    }

    #[test]
    fn browse_body_escapes_search_queries() {
        let query = "a \"quoted\" query \\ with a newline\ninside";
        let body = instance(None).browse_body("query", query);
        assert_eq!(body["query"], query);
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
        assert_eq!(body["context"]["client"]["clientVersion"], "1.0");
        let text = body.to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["query"], query);
        assert!(parsed.get("context").is_some());
    }

    #[test]
    fn browse_body_escapes_account_ids() {
        let body = instance(Some("a\"b\\c".to_string())).browse_body("browseId", "FEmusic_home");
        assert_eq!(body["browseId"], "FEmusic_home");
        assert_eq!(body["context"]["user"]["onBehalfOfUser"], "a\"b\\c");
        let text = body.to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["context"]["user"]["onBehalfOfUser"], "a\"b\\c");
    }

    #[test]
    fn innertube_url_percent_encodes_query_values() {
        let url = innertube_url("browse", &[("ctoken", "a&b=c+d"), ("key", "test-key")]).unwrap();
        assert_eq!(
            url.as_str(),
            "https://music.youtube.com/youtubei/v1/browse?ctoken=a%26b%3Dc%2Bd&key=test-key"
        );
    }
}
