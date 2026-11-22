//! HTTP stream reader for remote media sources.

use async_trait::async_trait;
use common_structs::TrackSource;
use recvrd_core::{StreamFactory, StreamReader};
use stream_download::{Settings, StreamDownload, storage::temp::TempStorageProvider};

/// Open HTTP streams with a temporary cache file.
pub struct HttpStreamFactory;

#[async_trait]
impl StreamFactory for HttpStreamFactory {
    async fn open(&self, source: &TrackSource) -> Result<StreamReader, String> {
        let url = source
            .url
            .parse()
            .map_err(|error| format!("invalid media URL: {error}"))?;
        let download =
            StreamDownload::new_http(url, TempStorageProvider::default(), Settings::default())
                .await
                .map_err(|error| error.to_string())?;
        let length = download.content_length().or(source.length);
        Ok(StreamReader {
            reader: Box::new(download),
            mime: source.mime.clone(),
            length,
            seekable: source.seekable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn range_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = b"0123456789abcdef".to_vec();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase();
                    let start = request
                        .lines()
                        .find_map(|line| line.strip_prefix("range: bytes="))
                        .and_then(|value| value.split('-').next())
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0);
                    let start = start.min(body.len());
                    let (status, extra) = if start > 0 {
                        (
                            "206 Partial Content",
                            format!(
                                "Content-Range: bytes {start}-{}/{}\r\n",
                                body.len() - 1,
                                body.len()
                            ),
                        )
                    } else {
                        ("200 OK", String::new())
                    };
                    let slice = &body[start..];
                    let header = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: audio/mpeg\r\n\
                         Content-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(slice).await;
                });
            }
        });
        format!("http://{address}/audio.mp3")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_and_seeks_a_remote_stream() {
        let url = range_server().await;
        let source = TrackSource {
            url,
            mime: Some("audio/mpeg".to_string()),
            length: Some(16),
            seekable: true,
            kind: common_structs::MediaKind::Audio,
        };
        let mut reader = HttpStreamFactory.open(&source).await.unwrap();
        let mut first = [0u8; 4];
        reader.reader.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"0123");
        reader.reader.seek(SeekFrom::Start(8)).unwrap();
        let mut second = [0u8; 4];
        reader.reader.read_exact(&mut second).unwrap();
        assert_eq!(&second, b"89ab");
    }
}
