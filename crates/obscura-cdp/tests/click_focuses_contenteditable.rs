// Clicking a contenteditable editing host focuses it, and contentEditable /
// isContentEditable report it (Playwright's fill() checks the latter), as in Chrome, so key
// and paste events reach it and execCommand('insertText') edits it. Only
// hosts with a tabindex (ProseMirror's) were focused before.

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
                let body = r#"<html><body style="margin:0"><div id="e" contenteditable="true" style="width:300px;height:40px">x<span id="in"></span><b id="off" contenteditable="false"></b></div><p id="plain"></p></body></html>"#;
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(resp.as_bytes()).await;
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
async fn click_focuses_a_contenteditable_host() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;
    for (id, kind) in [(2, "mousePressed"), (3, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type": kind, "x": 20, "y": 20, "button": "left", "clickCount": 1}), session_id).await;
    }
    let v = cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression": "document.activeElement.id + ':' + document.execCommand('insertText', false, 'y') + ':' + document.getElementById('e').textContent",
        "returnByValue": true,
    }), session_id).await;
    assert_eq!(v["result"]["value"].as_str(), Some("e:true:xy"));
    let editable = cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "expression": "JSON.stringify(['e', 'in', 'off', 'plain'].map(id => { const el = document.getElementById(id); return [el.contentEditable, el.isContentEditable]; }))",
        "returnByValue": true,
    }), session_id).await;
    assert_eq!(editable["result"]["value"].as_str(),
        Some(r#"[["true",true],["inherit",true],["false",false],["inherit",false]]"#),
        "contentEditable reflects the attribute; isContentEditable inherits it");
    // Input.insertText (Playwright's fill and keyboard.insertText) edits the host.
    cdp(&mut ctx, 6, "Input.insertText", json!({"text": "z"}), session_id).await;
    let typed = cdp(&mut ctx, 7, "Runtime.evaluate",
        json!({"expression": "document.getElementById('e').textContent", "returnByValue": true}), session_id).await;
    assert_eq!(typed["result"]["value"].as_str(), Some("xyz"));
}
