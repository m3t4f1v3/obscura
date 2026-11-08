// A parser-inserted `<link rel="preload" as="style">` must fire load once
// fetched, and switching a connected link's rel to "stylesheet" must load and
// apply it, firing load again with a sheet. chatgpt.com's logged-out page does
// exactly this (finding the link by comparing `link.href` with an absolute
// URL) and keeps every conversation turn hidden until that stylesheet loads.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..12 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..]);
                let (status, ct, body) = if req.starts_with("GET /style.css") {
                    ("200 OK", "text/css", "body { color: red; }")
                } else if req.starts_with("GET /later.css") {
                    ("200 OK", "text/css", "p { color: blue; }")
                } else {
                    (
                        "200 OK",
                        "text/html",
                        r#"<html><head>
<link rel="preload" href="/style.css" as="style">
<script>
window.__events = [];
var link = [...document.head.querySelectorAll('link[as="style"]')]
  .find(function (l) { return l.href === new URL('/style.css', document.baseURI).href; });
window.__found = !!link;
if (link) {
  link.addEventListener('load', function () {
    __events.push('load:' + link.rel + ':' + (link.sheet != null));
    if (link.rel !== 'stylesheet') link.rel = 'stylesheet';
  });
  link.addEventListener('error', function () { __events.push('error'); });
}
</script>
</head><body><p>x</p></body></html>"#,
                    )
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/")
}

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, session_id: &str) -> Value {
    let resp = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session_id.to_string()),
        },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

async fn eval(ctx: &mut CdpContext, id: u64, expression: &str, session_id: &str) -> String {
    let v = cdp(
        ctx,
        id,
        "Runtime.evaluate",
        json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
        session_id,
    )
    .await;
    v["result"]["value"].as_str().unwrap_or("").to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn parser_preload_link_fires_load_and_switches_to_stylesheet() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;

    // link.href reflects the resolved URL, so the page finds its link.
    let found = eval(&mut ctx, 2, "String(window.__found)", session_id).await;
    assert_eq!(found, "true", "link.href must be the absolute URL");

    let events = eval(
        &mut ctx,
        3,
        "(async () => { for (let i = 0; i < 50 && __events.length < 2; i++) await new Promise(r => setTimeout(r, 100)); return JSON.stringify(__events); })()",
        session_id,
    )
    .await;
    assert_eq!(
        events, r#"["load:preload:false","load:stylesheet:true"]"#,
        "the preload must fire load, then the rel switch must load the sheet (got {events:?})"
    );
    let sheets = eval(&mut ctx, 4, "String(document.styleSheets.length)", session_id).await;
    assert_eq!(sheets, "1", "the switched link must apply its stylesheet");

    // Changing a connected stylesheet link's href loads the new sheet.
    let reloaded = eval(
        &mut ctx,
        5,
        "(async () => { const l = document.querySelector('link'); const p = new Promise(r => { l.addEventListener('load', () => r('load'), {once: true}); l.addEventListener('error', () => r('error'), {once: true}); setTimeout(() => r('timeout'), 5000); }); l.setAttribute('href', '/later.css'); return await p; })()",
        session_id,
    )
    .await;
    assert_eq!(reloaded, "load", "an href change on a connected link must load it");
}
