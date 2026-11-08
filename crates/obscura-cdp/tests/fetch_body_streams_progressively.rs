// A fetch() body is readable as it arrives: a server-sent stream (chat
// replies, live logs) must reach `response.body` readers chunk by chunk, not
// only once the server closes it. Clients that see nothing while the server
// is still sending take the stream for stalled and abandon it.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..6 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..]);
                if req.starts_with("GET /stream") {
                    let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await;
                    for i in 0..3 {
                        let data = format!("data: part{i}\n\n");
                        let _ = socket.write_all(format!("{:x}\r\n{data}\r\n", data.len()).as_bytes()).await;
                        let _ = socket.flush().await;
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                    let _ = socket.write_all(b"0\r\n\r\n").await;
                } else {
                    let body = "<html><body>x</body></html>";
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = socket.write_all(resp.as_bytes()).await;
                }
            });
        }
    });
    format!("http://{addr}/")
}

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, session_id: &str) -> Value {
    let resp = dispatch(
        &CdpRequest { id, method: method.to_string(), params, session_id: Some(session_id.to_string()) },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_body_chunks_arrive_before_the_response_ends() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;

    let v = cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression": r#"(async () => {
            const t0 = Date.now();
            const reader = (await fetch('/stream')).body.getReader();
            const seen = [];
            for (;;) { const {done, value} = await reader.read(); if (done) break; seen.push([Date.now() - t0, new TextDecoder().decode(value)]); }
            const text = await (await fetch('/stream')).text();
            return JSON.stringify({seen, text, total: Date.now() - t0});
        })()"#,
        "awaitPromise": true, "returnByValue": true,
    }), session_id).await;
    let result: Value = serde_json::from_str(v["result"]["value"].as_str().unwrap()).unwrap();
    let seen = result["seen"].as_array().unwrap();
    assert_eq!(seen.len(), 3, "one read per network chunk: {result}");
    let first = seen[0][0].as_f64().unwrap();
    let last = seen[2][0].as_f64().unwrap();
    assert!(last - first >= 600.0, "chunks arrive as sent, not all at the end: {result}");
    assert_eq!(seen[0][1], "data: part0\n\n");
    assert_eq!(result["text"], "data: part0\n\ndata: part1\n\ndata: part2\n\n", "text() still reads it whole");
}
