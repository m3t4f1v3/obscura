// The command watchdog terminates a CDP command that overruns, cutting off
// whatever script was running, including an animation frame callback. That
// unwinds past `finally`, and the frame scheduler's running flag stayed set,
// so requestAnimationFrame never fired again on the page. Playwright's
// actionability checks wait for animation frames, so every later click timed
// out (seen on chatgpt.com after a long reply wait).

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await.unwrap();
                let body = "<html><body>x</body></html>";
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/")
}

async fn send(ctx: &mut CdpContext, id: u64, method: &str, params: Value) -> obscura_cdp::types::CdpResponse {
    dispatch(&CdpRequest { id, method: method.to_string(), params, session_id: Some("session-1".to_string()) }, ctx).await
}

#[tokio::test(flavor = "current_thread")]
async fn animation_frames_run_after_a_terminated_frame_callback() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    ctx.sessions.insert("session-1".to_string(), page_id.clone());
    let nav = send(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"})).await;
    assert!(nav.error.is_none(), "{:?}", nav.error);

    // A frame callback that spins until the watchdog terminates the command.
    std::env::set_var("OBSCURA_CDP_COMMAND_TIMEOUT_MS", "500");
    let _ = send(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression": "new Promise(resolve => requestAnimationFrame(() => { const end = Date.now() + 5000; while (Date.now() < end) {} resolve(1); }))",
        "awaitPromise": true, "returnByValue": true,
    })).await;
    std::env::set_var("OBSCURA_CDP_COMMAND_TIMEOUT_MS", "20000");

    let frame = send(&mut ctx, 3, "Runtime.evaluate", json!({
        "expression": "new Promise(resolve => { setTimeout(() => {}, 0); requestAnimationFrame(() => resolve('frame')); setTimeout(() => resolve('no frame'), 3000); })",
        "awaitPromise": true, "returnByValue": true,
    })).await;
    std::env::remove_var("OBSCURA_CDP_COMMAND_TIMEOUT_MS");
    assert!(frame.error.is_none(), "{:?}", frame.error);
    assert_eq!(frame.result.unwrap()["result"]["value"].as_str(), Some("frame"));
}
