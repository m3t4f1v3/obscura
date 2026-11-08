// `height: 100%` inside a flex item whose height comes from being stretched
// in a single-line row (CSS Flexbox 9.8 makes that height definite), and
// through `display: contents` wrappers (which generate no box, so the
// containing block is the next ancestor that does). App shells nest these down
// to their scroll containers; resolving the percentage as auto collapsed
// chatgpt.com's composer area to zero height, so clicks missed the composer.
// Expected heights are Chrome's.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PAGE: &str = r#"<!doctype html><html><head><style>html,body{margin:0;height:100%}</style></head><body>
<div id="frame2" style="display:flex;height:300px">
  <div id="stretched" style="display:flex;flex-direction:column"><div id="pct2" style="height:100%">x</div></div>
</div>
<div id="frame3" style="display:flex;flex-direction:column;height:200px">
  <div id="row3" style="display:flex;flex:1 1 0%;min-height:0">
    <div style="display:contents"><div style="display:contents">
      <main id="main3" style="display:flex;flex-direction:column;flex:1 1 0%">
        <div id="pct3" style="display:flex;flex-direction:column;height:100%"><div style="overflow:auto;flex:1 1 0%;min-height:0"><div style="height:500px">tall</div></div></div>
      </main>
    </div></div>
  </div>
</div>
<div id="frame4" style="display:flex;height:120px;align-items:flex-start">
  <div id="unstretched" style="display:flex;flex-direction:column"><div id="pct4" style="height:100%">x</div></div>
</div>
</body></html>"#;

async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await.unwrap();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                    PAGE.len()
                );
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
async fn percentage_heights_resolve_through_stretch_and_display_contents() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;
    let v = cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression": "JSON.stringify(Object.fromEntries(['pct2', 'pct3', 'pct4'].map(id => [id, Math.round(document.getElementById(id).getBoundingClientRect().height)])))",
        "returnByValue": true,
    }), session_id).await;
    let heights: Value = serde_json::from_str(v["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(heights["pct2"], 300, "a stretched row item's height is definite: {heights}");
    assert_eq!(heights["pct3"], 200, "display:contents passes the definite height through: {heights}");
    assert!(heights["pct4"].as_i64().unwrap() < 120, "a non-stretched item's height stays indefinite: {heights}");
}
