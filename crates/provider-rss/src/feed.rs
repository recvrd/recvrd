//! RSS 2.0 and Atom feed parsing for the RSS provider.
//!
//! The parser reads the fields the provider needs: feed title and
//! image, and one record per episode with its title, author,
//! duration, enclosure URL and cover image. It skips episodes
//! without an enclosure, since only enclosures can play.

use std::io::Cursor;

use quick_xml::{events::Event, reader::Reader};
use sha2::{Digest, Sha256};

/// One music feed: its title and image, and its episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    pub title: String,
    pub image_url: Option<String>,
    pub episodes: Vec<Episode>,
}

/// One feed episode. The identity is the episode GUID, or the
/// enclosure URL when the feed has no GUID. The identity never
/// changes for one episode, so a refresh cannot duplicate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    pub identity: String,
    pub title: String,
    pub author: String,
    pub duration: String,
    pub enclosure_url: String,
    pub enclosure_type: String,
    pub enclosure_length: Option<u64>,
    pub cover_url: Option<String>,
}

impl Episode {
    /// The audio extension for this enclosure, from its MIME type
    /// or its URL suffix. Empty when the type is not supported.
    pub fn extension(&self) -> Option<&'static str> {
        extension_for_enclosure(&self.enclosure_type, &self.enclosure_url)
    }
}

/// The feed ID for a feed URL: the first 16 characters of its
/// SHA-256 hash, in hexadecimal. The ID is stable for one URL.
pub fn feed_id(url: &str) -> String {
    hash_prefix(url, 16)
}

/// The track ID for an episode identity: the first 32 characters
/// of its SHA-256 hash, in hexadecimal. The ID is stable for one
/// episode and safe to use in a cache path.
pub fn episode_id(identity: &str) -> String {
    hash_prefix(identity, 32)
}

fn hash_prefix(value: &str, len: usize) -> String {
    let mut hash = Sha256::new();
    hash.update(value.as_bytes());
    let digest = hash.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    hex[..len].to_string()
}

/// The audio extension for an enclosure, from its MIME type first
/// and its URL suffix second. Empty when the type is not one of
/// the formats the daemon can play.
pub fn extension_for_enclosure(mime_type: &str, url: &str) -> Option<&'static str> {
    match mime_type {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/mp4" | "audio/x-m4a" | "audio/aac" => Some("m4a"),
        "audio/ogg" | "audio/vorbis" => Some("ogg"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        _ => {
            let path = url.split(['?', '#']).next().unwrap_or(url);
            let suffix = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            match suffix.as_str() {
                "mp3" => Some("mp3"),
                "m4a" => Some("m4a"),
                "ogg" => Some("ogg"),
                "flac" => Some("flac"),
                _ => None,
            }
        }
    }
}

/// Normalize a duration to the `M:SS` or `H:MM:SS` form the API
/// uses. Accepts seconds, `MM:SS` and `HH:MM:SS`. Empty when the
/// value is missing or invalid.
pub fn normalize_duration(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = raw.split(':').collect();
    let total: u64 = match parts.len() {
        1 => raw.parse().unwrap_or(0),
        2 => {
            let minutes: u64 = parts[0].parse().unwrap_or(0);
            let seconds: u64 = parts[1].parse().unwrap_or(0);
            minutes * 60 + seconds
        }
        3 => {
            let hours: u64 = parts[0].parse().unwrap_or(0);
            let minutes: u64 = parts[1].parse().unwrap_or(0);
            let seconds: u64 = parts[2].parse().unwrap_or(0);
            hours * 3600 + minutes * 60 + seconds
        }
        _ => return String::new(),
    };
    if total == 0 && parts.iter().any(|part| part.parse::<u64>().is_err()) {
        return String::new();
    }
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Parse RSS 2.0 or Atom feed bytes into a feed. The parser skips
/// episodes without an enclosure, since only enclosures can play.
pub fn parse_feed(bytes: &[u8]) -> Result<Feed, String> {
    let mut reader = Reader::from_reader(Cursor::new(bytes));
    // Keep raw text. Trimming here would drop spaces around
    // entities, so the parser trims each field once at its end tag.
    let mut parser = Parser::default();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = local_name(&event);
                parser.start(&name, &event);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(&event);
                parser.empty(&name, &event);
            }
            Ok(Event::End(event)) => {
                let name = event.local_name().into_inner().to_string();
                parser.end(&name);
            }
            Ok(Event::Text(event)) => {
                parser.text(&event.xml10_content());
            }
            Ok(Event::CData(event)) => {
                parser.text(&event.into_inner());
            }
            Ok(Event::GeneralRef(reference)) => {
                // A character reference such as &amp; arrives as
                // its own event. Decode it and treat it as text.
                let raw = format!("&{};", reference.into_inner());
                if let Ok(decoded) = quick_xml::escape::unescape(&raw) {
                    parser.text(&decoded);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("feed is not valid XML: {error}")),
        }
        buffer.clear();
    }
    if !parser.stack.is_empty() {
        return Err("feed is not valid XML: the document ends with open elements".to_string());
    }
    Ok(parser.finish())
}

fn local_name(event: &quick_xml::events::BytesStart) -> String {
    event.local_name().into_inner().to_string()
}

/// One episode under construction, plus the feed fields.
#[derive(Default)]
struct EpisodeBuilder {
    title: String,
    guid: String,
    author: String,
    duration: String,
    enclosure_url: String,
    enclosure_type: String,
    enclosure_length: Option<u64>,
    cover_url: Option<String>,
}

impl EpisodeBuilder {
    fn build(self) -> Option<Episode> {
        if self.enclosure_url.is_empty() {
            return None;
        }
        let identity = if self.guid.is_empty() {
            self.enclosure_url.clone()
        } else {
            self.guid
        };
        Some(Episode {
            identity,
            title: self.title,
            author: self.author,
            duration: normalize_duration(&self.duration),
            enclosure_url: self.enclosure_url,
            enclosure_type: self.enclosure_type,
            enclosure_length: self.enclosure_length,
            cover_url: self.cover_url,
        })
    }
}

#[derive(Default)]
struct Parser {
    stack: Vec<String>,
    title: String,
    image_url: Option<String>,
    episode: Option<EpisodeBuilder>,
    episodes: Vec<Episode>,
    /// The element the buffered text belongs to. quick-xml splits
    /// text at entities into separate events, so text for one
    /// element must accumulate and trim once at the end tag.
    text_target: Option<String>,
    text_buffer: String,
}

impl Parser {
    fn in_episode(&self) -> bool {
        self.stack
            .iter()
            .any(|name| name == "item" || name == "entry")
    }

    fn attribute(event: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
        event
            .attributes()
            .filter_map(|attribute| attribute.ok())
            .find(|attribute| attribute.key.as_ref() == key)
            .and_then(|attribute| {
                attribute
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
            })
            .map(|value| value.into_owned())
    }

    fn start(&mut self, name: &str, event: &quick_xml::events::BytesStart) {
        self.text_target = None;
        self.stack.push(name.to_string());
        if name == "item" || name == "entry" {
            self.episode = Some(EpisodeBuilder::default());
        }
        if name == "link" {
            let rel = Self::attribute(event, "rel").unwrap_or_default();
            if rel == "enclosure"
                && let Some(episode) = self.episode.as_mut()
            {
                episode.enclosure_url = Self::attribute(event, "href").unwrap_or_default();
                episode.enclosure_type = Self::attribute(event, "type").unwrap_or_default();
            }
        }
        if name == "image"
            && let Some(href) = Self::attribute(event, "href")
        {
            self.set_image(href);
        }
        if name == "thumbnail"
            && let Some(url) = Self::attribute(event, "url")
        {
            self.set_image(url);
        }
    }

    fn empty(&mut self, name: &str, event: &quick_xml::events::BytesStart) {
        if name == "enclosure"
            && let Some(episode) = self.episode.as_mut()
        {
            episode.enclosure_url = Self::attribute(event, "url").unwrap_or_default();
            episode.enclosure_type = Self::attribute(event, "type").unwrap_or_default();
            episode.enclosure_length =
                Self::attribute(event, "length").and_then(|value| value.parse().ok());
            return;
        }
        if name == "link" {
            let rel = Self::attribute(event, "rel").unwrap_or_default();
            if rel == "enclosure"
                && let Some(episode) = self.episode.as_mut()
            {
                episode.enclosure_url = Self::attribute(event, "href").unwrap_or_default();
                episode.enclosure_type = Self::attribute(event, "type").unwrap_or_default();
            }
            return;
        }
        if name == "image"
            && let Some(href) = Self::attribute(event, "href")
        {
            self.set_image(href);
            return;
        }
        if name == "thumbnail"
            && let Some(url) = Self::attribute(event, "url")
        {
            self.set_image(url);
        }
    }

    fn set_image(&mut self, url: String) {
        if self.in_episode() {
            if let Some(episode) = self.episode.as_mut()
                && episode.cover_url.is_none()
            {
                episode.cover_url = Some(url);
            }
        } else if self.image_url.is_none() {
            self.image_url = Some(url);
        }
    }

    fn text(&mut self, text: &str) {
        let Some(current) = self.stack.last().cloned() else {
            return;
        };
        // Collect raw text for text fields. Whitespace-only text
        // between elements has a container on top of the stack,
        // so it never lands here.
        let is_field = matches!(
            current.as_str(),
            "title"
                | "guid"
                | "id"
                | "author"
                | "creator"
                | "name"
                | "duration"
                | "url"
                | "icon"
                | "logo"
        );
        if !is_field {
            return;
        }
        if self.text_target.as_deref() != Some(current.as_str()) {
            self.text_buffer.clear();
            self.text_target = Some(current);
        }
        self.text_buffer.push_str(text);
    }

    /// Move the buffered text for one element into its field.
    fn flush_text(&mut self, name: &str) {
        if self.text_target.as_deref() != Some(name) {
            return;
        }
        let text = self.text_buffer.trim().to_string();
        self.text_buffer.clear();
        self.text_target = None;
        if self.in_episode() {
            let Some(episode) = self.episode.as_mut() else {
                return;
            };
            match name {
                "title" => episode.title = text,
                "guid" | "id" => episode.guid = text,
                "author" | "creator" | "name" => episode.author = text,
                "duration" => episode.duration = text,
                _ => {}
            }
            return;
        }
        match name {
            "title" => self.title = text,
            "url" => {
                if self.stack.iter().any(|name| name == "image") {
                    self.image_url = Some(text);
                }
            }
            "icon" | "logo" => self.image_url = Some(text),
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        self.flush_text(name);
        if (name == "item" || name == "entry")
            && let Some(episode) = self.episode.take()
            && let Some(built) = episode.build()
        {
            self.episodes.push(built);
        }
        self.stack.pop();
    }

    fn finish(self) -> Feed {
        Feed {
            title: self.title,
            image_url: self.image_url,
            episodes: self.episodes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = include_str!("../fixtures/rss.xml");
    const ATOM: &str = include_str!("../fixtures/atom.xml");

    #[test]
    fn feed_ids_are_stable_for_one_url() {
        let first = feed_id("https://example.com/feed.xml");
        assert_eq!(first.len(), 16);
        assert_eq!(first, feed_id("https://example.com/feed.xml"));
        assert_ne!(first, feed_id("https://example.com/other.xml"));
    }

    #[test]
    fn episode_ids_are_stable_for_one_identity() {
        let first = episode_id("episode-guid-1");
        assert_eq!(first.len(), 32);
        assert_eq!(first, episode_id("episode-guid-1"));
        assert_ne!(first, episode_id("episode-guid-2"));
    }

    #[test]
    fn durations_normalize_to_api_form() {
        assert_eq!(normalize_duration("245"), "4:05");
        assert_eq!(normalize_duration("4:05"), "4:05");
        assert_eq!(normalize_duration("1:04:05"), "1:04:05");
        assert_eq!(normalize_duration("0"), "0:00");
        assert_eq!(normalize_duration(""), "");
        assert_eq!(normalize_duration("nope"), "");
    }

    #[test]
    fn extension_comes_from_mime_type_first() {
        assert_eq!(
            extension_for_enclosure("audio/mpeg", "https://x/y"),
            Some("mp3")
        );
        assert_eq!(
            extension_for_enclosure("audio/mp4", "https://x/y"),
            Some("m4a")
        );
        assert_eq!(
            extension_for_enclosure("audio/ogg", "https://x/y"),
            Some("ogg")
        );
        assert_eq!(
            extension_for_enclosure("audio/flac", "https://x/y"),
            Some("flac")
        );
        assert_eq!(
            extension_for_enclosure("application/octet-stream", "https://x/y.mp3"),
            Some("mp3")
        );
        assert_eq!(
            extension_for_enclosure("application/octet-stream", "https://x/y.wav"),
            None
        );
        assert_eq!(
            extension_for_enclosure("", "https://x/y.flac"),
            Some("flac")
        );
    }

    #[test]
    fn parses_rss_with_itunes_fields() {
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(feed.title, "Test Tones");
        assert_eq!(feed.episodes.len(), 2);
        let first = &feed.episodes[0];
        assert_eq!(first.title, "First Song");
        assert_eq!(first.author, "The Testers");
        assert_eq!(first.duration, "4:05");
        let second = &feed.episodes[1];
        // XML entities in text decode to their characters.
        assert_eq!(second.title, "Second Song & More");
        assert_eq!(first.enclosure_url, "https://example.com/ep1.mp3");
        assert_eq!(first.enclosure_length, Some(12345));
        assert_eq!(first.extension(), Some("mp3"));
        assert_eq!(
            first.cover_url.as_deref(),
            Some("https://example.com/ep1.jpg")
        );
        assert_eq!(second.duration, "1:04:05");
        assert_eq!(second.enclosure_url, "https://example.com/ep2.flac");
        assert_eq!(second.extension(), Some("flac"));
    }

    #[test]
    fn parses_atom_with_enclosure_links() {
        let feed = parse_feed(ATOM.as_bytes()).unwrap();
        assert_eq!(feed.title, "Atom Tones");
        assert_eq!(feed.episodes.len(), 1);
        let episode = &feed.episodes[0];
        assert_eq!(episode.title, "Atom Song");
        assert_eq!(episode.author, "Atom Artist");
        assert_eq!(episode.enclosure_url, "https://example.com/atom.ogg");
        assert_eq!(episode.extension(), Some("ogg"));
    }

    #[test]
    fn episodes_without_enclosures_are_skipped() {
        let xml = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>F</title>
            <item><title>No audio</title><guid>g1</guid></item>
            <item><title>Has audio</title><guid>g2</guid>
            <enclosure url="https://example.com/a.mp3" type="audio/mpeg" length="1"/></item>
            </channel></rss>"#;
        let feed = parse_feed(xml.as_bytes()).unwrap();
        assert_eq!(feed.episodes.len(), 1);
        assert_eq!(feed.episodes[0].title, "Has audio");
    }

    #[test]
    fn invalid_xml_is_an_error() {
        assert!(parse_feed(b"<rss><channel>").is_err());
    }
}
