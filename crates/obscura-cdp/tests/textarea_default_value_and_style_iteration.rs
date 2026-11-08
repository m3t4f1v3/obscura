// `textarea.defaultValue` must exist and keep the parsed default once the
// value is edited; HTMLTextAreaElement.prototype must own its `value`
// descriptor (framework value trackers read it there); and a
// CSSStyleDeclaration must be iterable. chatgpt.com's logged-out composer
// threw on the first (`defaultValue.replace`) while binding the textarea, so
// sending never enabled, and on the last (`[...el.style]`) when sending.

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
                let body = r#"<html><body><form><textarea id="t">draft text</textarea></form>
<div id="d" style="color: red; margin-top: 4px"></div></body></html>"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
        json!({"expression": expression, "returnByValue": true}),
        session_id,
    )
    .await;
    v["result"]["value"].as_str().unwrap_or("").to_string()
}

#[tokio::test(flavor = "current_thread")]
async fn textarea_default_value_and_style_iteration() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;

    let own = eval(
        &mut ctx,
        2,
        "JSON.stringify(['value', 'defaultValue'].map(n => typeof Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, n)?.set))",
        session_id,
    )
    .await;
    assert_eq!(own, r#"["function","function"]"#, "textarea value/defaultValue must be own prototype accessors");

    let values = eval(
        &mut ctx,
        3,
        "(() => { const t = document.getElementById('t'); const before = [t.value, t.defaultValue]; t.value = 'typed'; const after = [t.value, t.defaultValue]; t.form.reset(); return JSON.stringify([before, after, t.value]); })()",
        session_id,
    )
    .await;
    assert_eq!(
        values,
        r#"[["draft text","draft text"],["typed","draft text"],"draft text"]"#,
        "defaultValue keeps the parsed default after an edit, and reset restores it (got {values:?})"
    );

    // Setting defaultValue on a clean textarea changes its value too.
    let clean = eval(
        &mut ctx,
        4,
        "(() => { const t = document.createElement('textarea'); t.defaultValue = 'seed'; return t.value + '|' + t.defaultValue; })()",
        session_id,
    )
    .await;
    assert_eq!(clean, "seed|seed");

    let names = eval(&mut ctx, 5, "JSON.stringify([...document.getElementById('d').style])", session_id).await;
    assert_eq!(names, r#"["color","margin-top"]"#, "a style declaration must iterate its property names");
}
