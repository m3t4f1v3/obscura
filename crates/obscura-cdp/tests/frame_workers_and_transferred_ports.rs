// Module scripts in frames, MessagePorts transferred across realms, and
// workers in their own realm. chatgpt.com renders replies in a sandboxed
// cross-origin frame that runs a module script, hands the page a port, and
// evaluates widgets in a worker that locks down its own global (deleting
// capabilities, freezing every intrinsic it can reach) before talking over a
// transferred port.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const WORKER: &str = r#"
onmessage = e => {
  const port = e.ports[0];
  // A sandboxed runtime's lockdown: remove capabilities and freeze intrinsics.
  const locationWas = String(location.href).slice(0, 5);
  // Assigning undefined first, as lockdowns do: it must not navigate the page.
  for (const key of ['queueMicrotask', 'document', 'location', 'console']) { Reflect.set(globalThis, key, undefined); delete globalThis[key]; }
  Object.freeze(Object.prototype);
  port.onmessage = m => {
    port.postMessage('worker:' + m.data + ':' + typeof queueMicrotask + ':' + typeof document + ':' + locationWas);
    // Binary data made in the worker's realm, transferred: it must reach the
    // page as an ArrayBuffer, not as a JSON-flattened {}.
    const buffer = new Uint8Array([7, 8, 9]).buffer;
    port.postMessage({buffer, when: new Date(0)}, [buffer]);
  };
};
"#;

async fn serve() -> (String, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    tokio::spawn(async move {
        for _ in 0..12 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..]);
                let (ct, body) = if req.starts_with("GET /child") {
                    ("text/html", format!(r#"<html><head><script type="module">
const worker = new Worker(URL.createObjectURL(new Blob([{worker}], {{type: 'text/javascript'}})));
addEventListener('message', ev => {{
  const port = ev.ports[0];
  if (!port) return;
  const channel = new MessageChannel();
  worker.postMessage('init', [channel.port2]);
  channel.port1.onmessage = m => port.postMessage(m.data);
  // Reply from this port's own handler: a posted task in the frame's realm.
  port.onmessage = m => {{ port.postMessage('frame:' + m.data); channel.port1.postMessage(m.data); }};
}});
parent.postMessage({{module: typeof import.meta.url}}, '*');
</script></head><body>child</body></html>"#, worker = serde_json::to_string(WORKER).unwrap()))
                } else {
                    ("text/html", format!(r#"<html><body><script>
window.got = [];
const frame = document.createElement('iframe');
frame.sandbox.add('allow-scripts');
frame.src = 'http://localhost:{port}/child';
addEventListener('message', ev => {{
  if (ev.source !== frame.contentWindow) return;
  got.push('module:' + ev.data.module);
  const channel = new MessageChannel();
  channel.port1.onmessage = m => got.push(typeof m.data === 'string' ? m.data
    : 'buffer:' + (m.data.buffer instanceof ArrayBuffer) + ':' + Array.from(new Uint8Array(m.data.buffer)).join(',') + ':' + (m.data.when instanceof Date));
  frame.contentWindow.postMessage('hello', '*', [channel.port2]);
  channel.port1.postMessage('ping');
}});
document.body.append(frame);
</script></body></html>"#))
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}/"), port)
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
async fn frame_module_ports_and_isolated_worker_realm() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let (url, _) = serve().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;

    let v = cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression": "(async () => { for (let i = 0; i < 60 && got.length < 4; i++) await new Promise(r => setTimeout(r, 100)); return JSON.stringify(got); })()",
        "awaitPromise": true, "returnByValue": true,
    }), session_id).await;
    let got: Vec<String> = serde_json::from_str(v["result"]["value"].as_str().unwrap_or("[]")).unwrap();
    assert!(got.contains(&"module:string".to_string()), "the frame's module script ran: {got:?}");
    assert!(got.contains(&"frame:ping".to_string()), "the transferred port reached the frame and its handler replied: {got:?}");
    assert!(
        got.contains(&"worker:ping:undefined:undefined:blob:".to_string()),
        "the frame's worker got a port, and deleting its globals took effect: {got:?}"
    );

    assert!(got.contains(&"buffer:true:7,8,9:true".to_string()),
        "binary data crosses worker, frame and page realms intact: {got:?}");

    // The worker froze its own Object.prototype, not the page's.
    let frozen = cdp(&mut ctx, 3, "Runtime.evaluate", json!({
        "expression": "String(Object.isFrozen(Object.prototype)) + ':' + typeof queueMicrotask + ':' + location.pathname",
        "returnByValue": true,
    }), session_id).await;
    assert_eq!(frozen["result"]["value"].as_str(), Some("false:function:/"),
        "the worker's lockdown froze and navigated nothing of the page's");
}
