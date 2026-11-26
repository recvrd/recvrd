use std::fmt::Display;

use common_structs::{ProviderId, TrackKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::YoutubeMusicPlaylistRef;

/// Applies recursively the `transformer` function to the given json value
/// and returns the transformed values.
pub(crate) fn from_json<T: PartialEq>(
    json: &Value,
    transformer: impl Fn(&Value) -> Option<T>,
) -> crate::Result<Vec<T>> {
    /// Execute a function on each element of a json value recursively.
    /// When the function returns something, the value is added to the result.
    pub(crate) fn inner_crawl<T: PartialEq>(
        value: &Value,
        playlists: &mut Vec<T>,
        transformer: &impl Fn(&Value) -> Option<T>,
    ) {
        if let Some(e) = transformer(value) {
            // Maybe an hashset would be better
            if !playlists.contains(&e) {
                playlists.push(e);
            }
            return;
        }
        match value {
            Value::Array(a) => a
                .iter()
                .for_each(|x| inner_crawl(x, playlists, transformer)),
            Value::Object(a) => a
                .values()
                .for_each(|x| inner_crawl(x, playlists, transformer)),
            _ => (),
        }
    }
    let mut playlists = Vec::new();
    inner_crawl(json, &mut playlists, &transformer);
    Ok(playlists)
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
pub struct YoutubeMusicVideoRef {
    pub title: String,
    pub author: String,
    pub album: String,
    pub video_id: String,
    pub duration: String,
    /// The provider this reference came from. Always YouTube here;
    /// old JSON sidecars without the field read as YouTube.
    #[serde(default)]
    pub provider: ProviderId,
    /// Unique ID of one playlist item. Mutations need it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_video_id: Option<String>,
    /// The like state from the item menu, when the page reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like_status: Option<LikeStatus>,
}

/// The like state that YouTube Music reports for one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LikeStatus {
    Like,
    Dislike,
    Indifferent,
}

impl YoutubeMusicVideoRef {
    /// The provider-scoped key of this reference.
    pub fn key(&self) -> TrackKey {
        TrackKey::new(self.provider, self.video_id.clone())
    }
}

impl Display for YoutubeMusicVideoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} | {}", self.author, self.title)
    }
}

/// Tries to extract a playlist from a json value.
/// Quite flexible to reduce odds of API change breaking this.
pub(crate) fn get_playlist(value: &Value) -> Option<YoutubeMusicPlaylistRef> {
    let object = value.as_object()?;
    let title_text = get_text(object.get("title")?, true, false)?;
    let subtitle = object
        .get("subtitle")
        .and_then(|x| get_text(x, false, false));
    let browse_id = &object
        .get("navigationEndpoint")
        .and_then(|x| x.get("browseEndpoint"))
        .and_then(|x| x.get("browseId"))
        .and_then(Value::as_str)?;
    Some(YoutubeMusicPlaylistRef {
        name: title_text,
        subtitle: subtitle.unwrap_or_default(),
        browse_id: browse_id.to_string(),
    })
}

#[derive(Debug, Clone, PartialOrd, Eq, Ord, PartialEq, Hash, Serialize, Deserialize)]
pub struct Continuation {
    pub(crate) continuation: String,
    pub(crate) click_tracking_params: String,
}

pub fn get_continuation(value: &Value) -> Option<Continuation> {
    let continuation = value
        .get("nextContinuationData")
        .and_then(|x| x.get("continuation"))
        .and_then(Value::as_str)?;
    let click_tracking_params = value
        .get("nextContinuationData")
        .and_then(|x| x.get("clickTrackingParams"))
        .and_then(Value::as_str)?;
    Some(Continuation {
        continuation: continuation.to_string(),
        click_tracking_params: click_tracking_params.to_string(),
    })
}

pub fn get_playlist_search(value: &Value) -> Option<YoutubeMusicPlaylistRef> {
    let browse_id = &value
        .get("navigationEndpoint")
        .and_then(|x| x.get("browseEndpoint"))
        .and_then(|x| x.get("browseId"))
        .and_then(Value::as_str)?;
    let titles: Vec<String> = value
        .get("flexColumns")?
        .as_array()?
        .iter()
        .flat_map(|x| {
            x.get("musicResponsiveListItemFlexColumnRenderer")
                .and_then(|x| x.get("text"))
                .and_then(|x| get_text(x, false, false))
        })
        .collect();
    Some(YoutubeMusicPlaylistRef {
        name: titles.first()?.clone(),
        subtitle: titles.get(1)?.clone(),
        browse_id: browse_id.to_string(),
    })
}

pub fn extract_playlist_info(value: &Value) -> Option<(String, String)> {
    // Playlist headers sit at the top level; album headers nest
    // under the tab contents, so search the whole page.
    if let Some(header) = find_renderer(
        value,
        &[
            "musicDetailHeaderRenderer",
            "musicEditablePlaylistDetailHeaderRenderer",
        ],
    ) {
        let title = get_text(header.get("title")?, false, false)?;
        let subtitles = subtitle_runs(header);
        return Some((title, subtitles.get(1)?.clone()));
    }
    let header = find_renderer(value, &["musicResponsiveHeaderRenderer"])?;
    let title = get_text(header.get("title")?, false, false)?;
    // The first subtitle run is the type ("Album"). The artist is
    // the first later run that is neither a year nor a track count.
    let artist = subtitle_runs(header)
        .into_iter()
        .skip(1)
        .find(|run| !is_year(run) && !is_track_count(run))
        .unwrap_or_default();
    Some((title, artist))
}

/// The page is an album page when its header is a
/// musicResponsiveHeaderRenderer. Playlist pages use a detail
/// header instead, and a detail header wins when both exist.
pub fn page_is_album(value: &Value) -> bool {
    if find_renderer(
        value,
        &[
            "musicDetailHeaderRenderer",
            "musicEditablePlaylistDetailHeaderRenderer",
        ],
    )
    .is_some()
    {
        return false;
    }
    find_renderer(value, &["musicResponsiveHeaderRenderer"]).is_some()
}

/// The album title of the page, when the page is an album page.
/// Playlist pages return None: the playlist name is not the album.
pub fn album_title(value: &Value) -> Option<String> {
    if !page_is_album(value) {
        return None;
    }
    extract_playlist_info(value).map(|(title, _)| title)
}
fn subtitle_runs(header: &Value) -> Vec<String> {
    header
        .get("subtitle")
        .and_then(|subtitle| subtitle.get("runs"))
        .and_then(Value::as_array)
        .map(|runs| {
            runs.iter()
                .flat_map(|run| get_text(run, false, false))
                .filter(|run| run != " • ")
                .collect()
        })
        .unwrap_or_default()
}

/// A year is four digits, such as "2025".
fn is_year(run: &str) -> bool {
    run.len() == 4 && run.chars().all(|c| c.is_ascii_digit())
}

/// A track count is a number followed by "songs", such as "12 songs".
fn is_track_count(run: &str) -> bool {
    run.split_once(' ').is_some_and(|(count, rest)| {
        !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) && rest.starts_with("song")
    })
}

/// Find the first renderer with one of the given keys. The search
/// is breadth-first: it checks each level before it goes deeper,
/// so a top-level header wins over a nested section header. A
/// nested header still matches when no shallower header exists.
fn find_renderer<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    use std::collections::VecDeque;
    let mut queue: VecDeque<&Value> = VecDeque::from([value]);
    while let Some(value) = queue.pop_front() {
        match value {
            Value::Object(map) => {
                for key in keys {
                    if let Some(found) = map.get(*key) {
                        return Some(found);
                    }
                }
                queue.extend(map.values());
            }
            Value::Array(items) => queue.extend(items),
            _ => {}
        }
    }
    None
}

/// Read the playlist or album header: the title and the subtitle.
/// A playlist subtitle is the second subtitle run. An album
/// subtitle is the artist, never the year. A missing header means
/// the page is not a playlist or album page. Albums nest the header
/// under the tab contents.
pub fn playlist_header_info(value: &Value) -> Option<(String, String)> {
    if let Some(info) = extract_playlist_info(value) {
        return Some(info);
    }
    // No subtitle runs: keep the title with an empty subtitle.
    let header = find_renderer(
        value,
        &[
            "musicDetailHeaderRenderer",
            "musicResponsiveHeaderRenderer",
            "musicEditablePlaylistDetailHeaderRenderer",
        ],
    )?;
    let title = get_text(header.get("title")?, false, false)?;
    Some((title, String::new()))
}

pub fn get_video_from_album(value: &Value) -> Option<YoutubeMusicVideoRef> {
    let video_id = value
        .get("playlistItemData")
        .and_then(|x| x.get("videoId"))
        .and_then(Value::as_str)?;
    video_from_flex(value, video_id.to_string(), false)
}

/// Tries to extract the text from a json value.
/// text_clean: Weather to include singleton text.
/// dot: Weather to use the dotted text instead of the space
fn get_text(value: &Value, text_clean: bool, dot: bool) -> Option<String> {
    if let Some(e) = value.as_str() {
        Some(e.to_string())
    } else {
        let obj = value.as_object()?;
        if let Some(e) = obj.get("text") {
            if text_clean && obj.values().count() == 1 {
                return None;
            }
            get_text(e, text_clean, dot)
        } else if let Some(e) = obj.get("runs") {
            let k = e
                .as_array()?
                .iter()
                .flat_map(|x| get_text(x, text_clean, dot))
                .collect::<Vec<_>>();
            if k.is_empty() {
                None
            } else {
                Some(join_clean(&k, dot))
            }
        } else {
            None
        }
    }
}

fn join_clean(strings: &[String], dot: bool) -> String {
    strings
        .iter()
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(if dot { " • " } else { " " })
}

/// Tries to find a video id in the json
pub fn get_videoid(value: &Value) -> Option<String> {
    match value {
        Value::Array(e) => e.iter().find_map(get_videoid),
        Value::Object(e) => e
            .get("videoId")
            .and_then(Value::as_str)
            .map(|x| x.to_string())
            .or_else(|| e.values().find_map(get_videoid)),
        _ => None,
    }
}

/// Tries to find a playlist item ID in the json.
fn get_set_video_id(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.iter().find_map(get_set_video_id),
        Value::Object(map) => map
            .get("setVideoId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| map.values().find_map(get_set_video_id)),
        _ => None,
    }
}

/// Tries to extract a video from a json value.
/// Quite flexible to reduce odds of API change breaking this.
pub(crate) fn get_video(value: &Value) -> Option<YoutubeMusicVideoRef> {
    video_from_flex(value, get_videoid(value)?, true)
}

/// Read one flex column, including `simpleText`.
fn flex_column_text(value: &Value) -> Option<String> {
    let renderer = value.as_object()?.values().next()?;
    let text = renderer.get("text")?;
    if let Some(simple) = text.get("simpleText").and_then(Value::as_str) {
        return Some(simple.to_string());
    }
    get_text(text, false, true)
}

/// Read every flex column text in order.
fn flex_column_texts(value: &Value) -> Option<Vec<String>> {
    Some(
        value
            .get("flexColumns")?
            .as_array()?
            .iter()
            .filter_map(flex_column_text)
            .collect(),
    )
}

/// Read every fixed column text in order.
fn fixed_column_texts(value: &Value) -> Vec<String> {
    value
        .get("fixedColumns")
        .and_then(Value::as_array)
        .map(|columns| columns.iter().filter_map(flex_column_text).collect())
        .unwrap_or_default()
}

/// True for a duration such as `3:45` or `1:02:03`.
fn is_duration(text: &str) -> bool {
    let parts: Vec<_> = text.split(':').collect();
    (parts.len() == 2 || parts.len() == 3)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Build a track from the flex columns of one playlist or album item.
fn video_from_flex(
    value: &Value,
    video_id: String,
    require_author: bool,
) -> Option<YoutubeMusicVideoRef> {
    let texts = flex_column_texts(value)?;
    let title = texts.first()?.clone();
    let author = match texts.get(1) {
        Some(author) => author.clone(),
        None if require_author => return None,
        None => String::new(),
    };
    let mut album = String::new();
    let mut duration = String::new();
    for text in texts.iter().skip(2) {
        if is_duration(text) && duration.is_empty() {
            duration = text.clone();
        } else if album.is_empty() {
            album = text.clone();
        }
    }
    for text in fixed_column_texts(value) {
        if is_duration(&text) && duration.is_empty() {
            duration = text;
        }
    }
    Some(YoutubeMusicVideoRef {
        title,
        author,
        album,
        video_id,
        duration,
        provider: ProviderId::Youtube,
        set_video_id: get_set_video_id(value),
        like_status: get_like_status(value),
    })
}

/// Tries to find the like state in the json item menu.
fn get_like_status(value: &Value) -> Option<LikeStatus> {
    match value {
        Value::Array(items) => items.iter().find_map(get_like_status),
        Value::Object(map) => {
            if let Some(status) = map.get("likeStatus").and_then(Value::as_str) {
                return match status {
                    "LIKE" => Some(LikeStatus::Like),
                    "DISLIKE" => Some(LikeStatus::Dislike),
                    "INDIFFERENT" => Some(LikeStatus::Indifferent),
                    _ => None,
                };
            }
            map.values().find_map(get_like_status)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search_video() -> Value {
        json!({
            "videoId": "vid123",
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Song"] },
                    "trackingParams": "a"
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Artist"] },
                    "trackingParams": "b"
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Album"] },
                    "trackingParams": "c"
                }}
            ]
        })
    }

    fn playlist() -> Value {
        json!({
            "title": { "runs": ["My Playlist"] },
            "subtitle": { "runs": ["Artist"] },
            "navigationEndpoint": {
                "browseEndpoint": { "browseId": "VLPL123" }
            }
        })
    }

    #[test]
    fn extracts_video_from_search_result() {
        let video = get_video(&search_video()).expect("video");
        assert_eq!(video.video_id, "vid123");
        assert_eq!(video.title, "Song");
        assert_eq!(video.author, "Artist");
        assert_eq!(video.album, "Album");
        assert_eq!(video.set_video_id, None);
    }

    #[test]
    fn extracts_a_playlist_item_duration() {
        let value = json!({
            "playlistItemData": {"videoId": "vid123"},
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Song"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Artist"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "simpleText": "3:45" }
                }}
            ]
        });
        let video = get_video(&value).expect("video");
        assert_eq!(video.title, "Song");
        assert_eq!(video.author, "Artist");
        assert_eq!(video.album, "");
        assert_eq!(video.duration, "3:45");
    }

    #[test]
    fn extracts_album_and_duration_from_four_columns() {
        let value = json!({
            "playlistItemData": {"videoId": "vid123"},
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Song"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Artist"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Album"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "simpleText": "3:45" }
                }}
            ]
        });
        let video = get_video(&value).expect("video");
        assert_eq!(video.album, "Album");
        assert_eq!(video.duration, "3:45");
    }

    #[test]
    fn extracts_a_fixed_column_duration() {
        let value = json!({
            "playlistItemData": {"videoId": "vid123"},
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Song"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Artist"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Album"] }
                }}
            ],
            "fixedColumns": [
                { "musicResponsiveListItemFixedColumnRenderer": {
                    "text": { "simpleText": "3:45" }
                }}
            ]
        });
        let video = get_video(&value).expect("video");
        assert_eq!(video.album, "Album");
        assert_eq!(video.duration, "3:45");
    }

    #[test]
    fn extracts_an_album_track_duration() {
        let value = json!({
            "playlistItemData": { "videoId": "albumvid" },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Album Song"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Artist"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "simpleText": "3:45" }
                }}
            ]
        });
        let video = get_video_from_album(&value).expect("video");
        assert_eq!(video.title, "Album Song");
        assert_eq!(video.duration, "3:45");
    }

    #[test]
    fn extracts_playlist_item_id() {
        let mut value = search_video();
        value["menu"] = json!({
            "menuRenderer": {
                "items": [{
                    "menuServiceItemRenderer": {
                        "serviceEndpoint": {
                            "playlistEditEndpoint": {
                                "actions": [{
                                    "setVideoId": "set-1",
                                    "removedVideoId": "vid123",
                                    "action": "ACTION_REMOVE_VIDEO"
                                }]
                            }
                        }
                    }
                }]
            }
        });

        let video = get_video(&value).expect("video");
        assert_eq!(video.set_video_id.as_deref(), Some("set-1"));
    }

    #[test]
    fn extracts_playlist() {
        let playlist = get_playlist(&playlist()).expect("playlist");
        assert_eq!(playlist.name, "My Playlist");
        assert_eq!(playlist.subtitle, "Artist");
        assert_eq!(playlist.browse_id, "VLPL123");
    }

    #[test]
    fn extracts_playlist_header_info() {
        let value = json!({
            "header": {"musicDetailHeaderRenderer": {
                "title": {"runs": [{"text": "My Mix"}]},
                "subtitle": {"runs": [
                    {"text": "Playlist"},
                    {"text": " • "},
                    {"text": "12 songs"}
                ]}
            }}
        });
        assert_eq!(
            playlist_header_info(&value),
            Some(("My Mix".to_string(), "12 songs".to_string()))
        );
        assert_eq!(playlist_header_info(&json!({"contents": {}})), None);
    }

    #[test]
    fn extracts_an_album_header_info() {
        // Albums nest the header under the tab contents and use
        // musicResponsiveHeaderRenderer. The subtitle is the artist
        // here, even though the year comes first.
        let value = json!({
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{
                "tabRenderer": {"content": {"sectionListRenderer": {"contents": [{
                    "musicResponsiveHeaderRenderer": {
                        "title": {"runs": [{"text": "An Album"}]},
                        "subtitle": {"runs": [
                            {"text": "Album"},
                            {"text": " • "},
                            {"text": "2025"},
                            {"text": " • "},
                            {"text": "The Artist"}
                        ]}
                    }
                }]}}}
            }]}}
        });
        assert_eq!(
            playlist_header_info(&value),
            Some(("An Album".to_string(), "The Artist".to_string()))
        );
    }

    #[test]
    fn album_header_without_an_artist_keeps_an_empty_subtitle() {
        // The subtitle lists only the type and the year, so no
        // artist exists to show. The year is not the subtitle.
        let value = json!({
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{
                "tabRenderer": {"content": {"sectionListRenderer": {"contents": [{
                    "musicResponsiveHeaderRenderer": {
                        "title": {"runs": [{"text": "An Album"}]},
                        "subtitle": {"runs": [
                            {"text": "Album"},
                            {"text": " • "},
                            {"text": "2025"}
                        ]}
                    }
                }]}}}
            }]}}
        });
        assert_eq!(
            playlist_header_info(&value),
            Some(("An Album".to_string(), String::new()))
        );
    }

    #[test]
    fn page_is_album_reads_the_header_kind() {
        let album = json!({
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{
                "tabRenderer": {"content": {"sectionListRenderer": {"contents": [{
                    "musicResponsiveHeaderRenderer": {
                        "title": {"runs": [{"text": "An Album"}]}
                    }
                }]}}}
            }]}}
        });
        assert!(page_is_album(&album));
        assert_eq!(album_title(&album), Some("An Album".to_string()));

        let playlist = json!({
            "header": {"musicDetailHeaderRenderer": {
                "title": {"runs": [{"text": "My Mix"}]}
            }}
        });
        assert!(!page_is_album(&playlist));
        assert_eq!(album_title(&playlist), None);

        assert!(!page_is_album(&json!({"contents": {}})));
        assert_eq!(album_title(&json!({"contents": {}})), None);
    }

    #[test]
    fn playlist_header_without_a_subtitle_keeps_the_title() {
        let value = json!({
            "header": {"musicDetailHeaderRenderer": {
                "title": {"runs": [{"text": "No Subtitle"}]}
            }}
        });
        assert_eq!(
            playlist_header_info(&value),
            Some(("No Subtitle".to_string(), String::new()))
        );
    }

    #[test]
    fn extracts_album_playlist_info() {
        // The album subtitle lists the type, the artist, and the
        // year. The fill takes the artist, never the year.
        let value = json!({
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{
                "tabRenderer": {"content": {"sectionListRenderer": {"contents": [{
                    "musicResponsiveHeaderRenderer": {
                        "title": {"runs": [{"text": "An Album"}]},
                        "subtitle": {"runs": [
                            {"text": "Album"},
                            {"text": " • "},
                            {"text": "The Artist"},
                            {"text": " • "},
                            {"text": "2025"}
                        ]}
                    }
                }]}}}
            }]}}
        });
        assert_eq!(
            extract_playlist_info(&value),
            Some(("An Album".to_string(), "The Artist".to_string()))
        );
    }

    #[test]
    fn extracts_album_playlist_info_with_year_before_artist() {
        // The order of the artist and the year varies, so the fill
        // skips the year wherever it sits.
        let value = json!({
            "contents": {"twoColumnBrowseResultsRenderer": {"tabs": [{
                "tabRenderer": {"content": {"sectionListRenderer": {"contents": [{
                    "musicResponsiveHeaderRenderer": {
                        "title": {"runs": [{"text": "An Album"}]},
                        "subtitle": {"runs": [
                            {"text": "Album"},
                            {"text": " • "},
                            {"text": "2025"},
                            {"text": " • "},
                            {"text": "The Artist"}
                        ]}
                    }
                }]}}}
            }]}}
        });
        assert_eq!(
            extract_playlist_info(&value),
            Some(("An Album".to_string(), "The Artist".to_string()))
        );
    }

    #[test]
    fn extracts_playlist_info_unchanged() {
        let value = json!({
            "header": {"musicDetailHeaderRenderer": {
                "title": {"runs": [{"text": "My Mix"}]},
                "subtitle": {"runs": [
                    {"text": "Playlist"},
                    {"text": " • "},
                    {"text": "The Author"}
                ]}
            }}
        });
        assert_eq!(
            extract_playlist_info(&value),
            Some(("My Mix".to_string(), "The Author".to_string()))
        );
    }

    #[test]
    fn find_renderer_prefers_the_shallowest_match() {
        let value = json!({
            "header": {"musicDetailHeaderRenderer": {
                "title": {"runs": [{"text": "Top Header"}]}
            }},
            "contents": {"sectionListRenderer": {"contents": [{
                "musicDetailHeaderRenderer": {
                    "title": {"runs": [{"text": "Nested Header"}]}
                }
            }]}}
        });
        let header = find_renderer(&value, &["musicDetailHeaderRenderer"]).unwrap();
        let title = get_text(header.get("title").unwrap(), false, false);
        assert_eq!(title.as_deref(), Some("Top Header"));
    }

    #[test]
    fn find_renderer_finds_a_nested_match() {
        let value = json!({
            "contents": {"sectionListRenderer": {"contents": [{
                "musicDetailHeaderRenderer": {
                    "title": {"runs": [{"text": "Nested Header"}]}
                }
            }]}}
        });
        assert!(find_renderer(&value, &["musicDetailHeaderRenderer"]).is_some());
    }

    #[test]
    fn extracts_search_playlist() {
        let value = json!({
            "navigationEndpoint": {
                "browseEndpoint": { "browseId": "VLPL456" }
            },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Search Playlist"] }
                }},
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Artist"] }
                }}
            ]
        });
        let playlist = get_playlist_search(&value).expect("playlist");
        assert_eq!(playlist.name, "Search Playlist");
        assert_eq!(playlist.subtitle, "Artist");
        assert_eq!(playlist.browse_id, "VLPL456");
    }

    #[test]
    fn extracts_continuation() {
        let value = json!({
            "nextContinuationData": {
                "continuation": "token",
                "clickTrackingParams": "ctp"
            }
        });
        let continuation = get_continuation(&value).expect("continuation");
        assert_eq!(continuation.continuation, "token");
        assert_eq!(continuation.click_tracking_params, "ctp");
    }

    #[test]
    fn finds_nested_video_id() {
        let value = json!({ "a": [{ "b": { "videoId": "nested" } }] });
        assert_eq!(get_videoid(&value), Some("nested".to_string()));
        assert_eq!(get_videoid(&json!({ "a": 1 })), None);
    }

    #[test]
    fn deduplicates_crawled_values() {
        let value = json!([playlist(), playlist()]);
        let playlists = from_json(&value, get_playlist).expect("playlists");
        assert_eq!(playlists.len(), 1);
    }

    #[test]
    fn extracts_the_like_status_from_the_menu() {
        let mut value = search_video();
        value["menu"] = json!({
            "topLevelButtons": [{
                "likeButtonRenderer": { "likeStatus": "LIKE" }
            }]
        });
        let video = get_video(&value).expect("video");
        assert_eq!(video.like_status, Some(LikeStatus::Like));
    }

    #[test]
    fn an_unknown_like_status_is_none() {
        let mut value = search_video();
        value["menu"] = json!({
            "topLevelButtons": [{
                "likeButtonRenderer": { "likeStatus": "UNKNOWN" }
            }]
        });
        let video = get_video(&value).expect("video");
        assert_eq!(video.like_status, None);
    }

    #[test]
    fn extracts_album_track() {
        let value = json!({
            "playlistItemData": { "videoId": "albumvid" },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": {
                    "text": { "runs": ["Album Song"] }
                }}
            ]
        });
        let video = get_video_from_album(&value).expect("video");
        assert_eq!(video.video_id, "albumvid");
        assert_eq!(video.title, "Album Song");
        assert_eq!(video.set_video_id, None);
    }
}
