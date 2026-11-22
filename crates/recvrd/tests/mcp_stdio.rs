use axum::{Json, Router, routing::get};
use recvrd_api::{
    AuthState, AuthStatus, DownloadSummary, PlaybackSnapshot, PlaybackState, ProviderId, Status,
    Track,
};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{ChildStdin, ChildStdout},
    time::timeout,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

async fn mock_daemon() -> String {
    let status = Status {
        name: "recvrd".to_string(),
        version: "0.4.0".to_string(),
        status: "ok".to_string(),
        uptime_seconds: 1,
        auth: AuthStatus {
            state: AuthState::Ok,
            source: None,
            message: None,
        },
        playback: PlaybackSnapshot {
            state: PlaybackState::Playing,
            current: Some(Track {
                provider: ProviderId::Youtube,
                track_id: "v1".to_string(),
                title: "Song".to_string(),
                artist: "Artist".to_string(),
                album: "Album".to_string(),
                duration: "3:00".to_string(),
                ..Default::default()
            }),
            position_secs: 12.0,
            volume: 50,
            repeat: Default::default(),
            shuffle: false,
        },
        queue_length: 1,
        downloads: DownloadSummary::default(),
        providers: vec![ProviderId::Youtube],
        default_provider: ProviderId::Youtube,
    };
    let router = Router::new().route(
        "/api/status",
        get(move || {
            let status = status.clone();
            async move { Json(status) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{address}")
}

async fn send(stdin: &mut ChildStdin, message: Value) {
    stdin
        .write_all(format!("{message}\n").as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
}

async fn read_response(lines: &mut Lines<BufReader<ChildStdout>>, id: u64) -> Value {
    loop {
        let line = timeout(RESPONSE_TIMEOUT, lines.next_line())
            .await
            .expect("MCP response timeout")
            .expect("MCP stdout read failed");
        let Some(line) = line else {
            panic!("MCP stdout closed before response {id}");
        };
        if line.trim().is_empty() {
            continue;
        }
        let response: Value = serde_json::from_str(&line).expect("valid JSON response");
        if response["id"] == id {
            return response;
        }
    }
}

#[tokio::test]
async fn mcp_stdio_lists_tools_and_exits_on_eof() {
    let base = mock_daemon().await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_recvrd"))
        .args(["mcp", "--url", &base])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn recvrd mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = BufReader::new(stdout).lines();

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0.0.0" }
            }
        }),
    )
    .await;
    let initialize = read_response(&mut lines, 1).await;
    assert_eq!(initialize["result"]["protocolVersion"], "2025-06-18");

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    )
    .await;
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    )
    .await;
    let tools = read_response(&mut lines, 2).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 33);
    assert!(names.contains(&"search"));
    assert!(names.contains(&"download_status"));
    assert!(names.contains(&"create_playlist"));
    assert!(names.contains(&"seek"));
    assert!(names.contains(&"queue_clear"));
    assert!(names.contains(&"list_providers"));
    assert!(names.contains(&"rss_list_feeds"));
    assert!(names.contains(&"rss_add_feed"));
    assert!(names.contains(&"rss_refresh_feeds"));

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "now_playing", "arguments": {} }
        }),
    )
    .await;
    let call = read_response(&mut lines, 3).await;
    assert_eq!(
        call["result"]["structuredContent"]["playback"]["volume"],
        50
    );
    assert_eq!(
        call["result"]["structuredContent"]["playback"]["current"]["track_id"],
        "v1"
    );
    assert_eq!(
        call["result"]["structuredContent"]["playback"]["current"]["provider"],
        "youtube"
    );

    drop(stdin);
    let status = timeout(EXIT_TIMEOUT, child.wait())
        .await
        .expect("MCP process did not exit")
        .unwrap();
    assert!(status.success(), "MCP process failed: {status}");
}
