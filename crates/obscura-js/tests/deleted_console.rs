// A page, or a sandboxed worker locking down its global, may delete
// `console`. Obscura's own error reporting must not depend on that binding:
// a throwing listener was reported with `console.error(...)` from inside the
// catch, which then threw a ReferenceError out of dispatch.
use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::json;

#[test]
fn listener_errors_are_contained_after_console_is_deleted() {
    let mut rt = ObscuraJsRuntime::new();
    rt.set_dom(obscura_dom::parse_html("<body></body>"));
    rt.run_page_init();
    let result = rt
        .evaluate(
            r#"(() => {
        delete globalThis.console;
        const seen = [];
        addEventListener('probe', () => { throw new Error('boom'); });
        addEventListener('probe', () => seen.push('second listener ran'));
        let escaped = null;
        try { dispatchEvent(new Event('probe')); } catch (e) { escaped = e.name; }
        return [typeof console, escaped, seen];
    })()"#,
        )
        .unwrap();
    assert_eq!(result, json!(["undefined", null, ["second listener ran"]]));
}
