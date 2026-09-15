use super::native::{NativeRequestHandler, NativeState, NativeWindow};
use objc2::{
    AnyThread, DeclaredClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::ProtocolObject,
};
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSObject, NSObjectProtocol, NSString,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationResponse, WKNavigationResponsePolicy, WKScriptMessage, WKScriptMessageHandler,
    WKURLSchemeHandler, WKURLSchemeTask, WKUserContentController, WKWebView,
};
use serde::Deserialize;
use std::{
    cell::Cell,
    sync::{Weak, atomic::Ordering},
};

pub(super) const SCHEME: &str = "antiburn-anchored";

#[derive(Clone)]
pub(super) struct BridgePolicy {
    pub entry: url::Url,
}
impl BridgePolicy {
    pub fn new(app: &tauri::AppHandle, route: &str) -> tauri::Result<Self> {
        let base = if tauri::is_dev() {
            app.config().build.dev_url.clone()
        } else {
            None
        };
        let base = base.unwrap_or_else(|| {
            url::Url::parse("antiburn-anchored://localhost/")
                .expect("the built-in preview URL is valid")
        });
        let entry = base
            .join(route)
            .map_err(|error| tauri::Error::Io(std::io::Error::other(error)))?;
        Ok(Self { entry })
    }
    fn allows_document(&self, value: &str) -> bool {
        let Ok(url) = url::Url::parse(value) else {
            return false;
        };
        url.scheme() == self.entry.scheme()
            && url.host_str() == self.entry.host_str()
            && url.port_or_known_default() == self.entry.port_or_known_default()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == self.entry.path()
            && url.query() == self.entry.query()
    }
}

fn asset_path(raw: &str) -> Option<&str> {
    let raw = raw.strip_prefix("antiburn-anchored://localhost/")?;
    if raw.contains(['%', '?', '#', '\\'])
        || raw
            .split('/')
            .any(|part| part == ".." || part == "." || part.is_empty())
    {
        return None;
    }
    (raw == "native-peek.html" || raw.starts_with("assets/")).then_some(raw)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    id: u64,
    renderer_generation: u64,
    command: String,
    args: serde_json::Value,
}
impl Request {
    fn parse(body: &str, generation: u64) -> Option<Self> {
        if body.len() > 16_384 {
            return None;
        }
        let request: Self = serde_json::from_str(body).ok()?;
        (request.id > 0
            && request.id <= 9_007_199_254_740_991
            && request.renderer_generation == generation
            && request.args.is_object()
            && matches!(
                request.command.as_str(),
                "get_popover_peek_state"
                    | "get_popover_peek_data"
                    | "popover_peek_ready"
                    | "popover_peek_presented"
                    | "popover_peek_retarget_ready"
                    | "popover_peek_concealed"
                    | "note_interaction"
            ))
        .then_some(request)
    }
}

pub(super) struct DelegateState {
    owner: Weak<NativeState>,
    policy: BridgePolicy,
    generation: u64,
    handler: NativeRequestHandler,
    webview: Cell<*const WKWebView>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    pub(super) struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl WKScriptMessageHandler for Delegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: WebKit delivers these objects on the main thread.
            unsafe {
                let state = self.ivars();
                let Some(owner) = state.owner.upgrade() else {
                    return;
                };
                if !owner.alive.load(Ordering::Acquire) {
                    return;
                }
                let Some(webview) = message.webView() else {
                    return;
                };
                if !std::ptr::eq(&*webview, state.webview.get()) {
                    return;
                }
                let frame = message.frameInfo();
                if !frame.isMainFrame() {
                    return;
                }
                let Some(url) = frame.request().URL().and_then(|url| url.absoluteString()) else {
                    return;
                };
                if !state.policy.allows_document(&url.to_string()) {
                    return;
                }
                let Ok(body) = message.body().downcast::<NSString>() else {
                    return;
                };
                let Some(request) = Request::parse(&body.to_string(), state.generation) else {
                    return;
                };
                let handler = state.handler;
                let weak = state.owner.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let Some(owner) = weak.upgrade() else {
                        return;
                    };
                    if !owner.alive.load(Ordering::Acquire) {
                        return;
                    }
                    let result = handler(&owner.app, &request.command, request.args);
                    let response = match result {
                        Ok(value) => serde_json::json!({"id":request.id,"value":value}),
                        Err(error) => serde_json::json!({"id":request.id,"error":error}),
                    };
                    let _ = NativeWindow(owner)
                        .eval(format!("window.__ANTIBURN_NATIVE_DELIVER__?.({response})"));
                });
            }
        }
    }
    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn navigation(
            &self,
            _view: &WKWebView,
            action: &WKNavigationAction,
            decision: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: The action and decision block are valid for this delegate callback.
            unsafe {
                let allowed = action
                    .targetFrame()
                    .is_some_and(|frame| frame.isMainFrame())
                    && action
                        .request()
                        .URL()
                        .and_then(|url| url.absoluteString())
                        .is_some_and(|url| self.ivars().policy.allows_document(&url.to_string()));
                decision.call((if allowed {
                    WKNavigationActionPolicy::Allow
                } else {
                    WKNavigationActionPolicy::Cancel
                },));
            }
        }
        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn response(
            &self,
            _view: &WKWebView,
            response: &WKNavigationResponse,
            decision: &block2::Block<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            // SAFETY: WebKit owns this response and decision block during the callback.
            unsafe {
                decision.call((if response.canShowMIMEType() {
                    WKNavigationResponsePolicy::Allow
                } else {
                    WKNavigationResponsePolicy::Cancel
                },));
            }
        }
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn terminated(&self, _view: &WKWebView) {
            self.fail();
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn load_failed(
            &self,
            _view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            tracing::warn!(error = %error, "native companion navigation failed");
            self.fail();
        }
    }
    unsafe impl WKURLSchemeHandler for Delegate {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_task(&self, _view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            // SAFETY: The scheme task remains active throughout this synchronous callback.
            unsafe {
                let Some(owner) = self.ivars().owner.upgrade() else {
                    return;
                };
                let Some(url) = task.request().URL() else {
                    return;
                };
                let raw = url
                    .absoluteString()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let asset = asset_path(&raw)
                    .and_then(|path| owner.app.asset_resolver().get(path.to_string()));
                let (status, bytes, mime) = match asset {
                    Some(asset) => (200, asset.bytes, asset.mime_type),
                    None => (404, Vec::new(), "text/plain".into()),
                };
                let names = [
                    NSString::from_str("Content-Type"),
                    NSString::from_str("Content-Security-Policy"),
                    NSString::from_str("Cache-Control"),
                ];
                let values = [
                    NSString::from_str(&mime),
                    NSString::from_str(
                        "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'",
                    ),
                    NSString::from_str("no-store"),
                ];
                let headers = NSDictionary::from_slices(
                    &[&*names[0], &*names[1], &*names[2]],
                    &[&*values[0], &*values[1], &*values[2]],
                );
                if let Some(response) =
                    NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
                        NSHTTPURLResponse::alloc(),
                        &url,
                        status,
                        None,
                        Some(&headers),
                    )
                {
                    task.didReceiveResponse(&response);
                    task.didReceiveData(&NSData::with_bytes(&bytes));
                    task.didFinish();
                }
            }
        }
        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_task(&self, _view: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
    }
);

impl Delegate {
    pub fn new(
        mtm: MainThreadMarker,
        owner: Weak<NativeState>,
        policy: BridgePolicy,
        generation: u64,
        handler: NativeRequestHandler,
    ) -> Retained<Self> {
        let allocated = Self::alloc(mtm).set_ivars(DelegateState {
            owner,
            policy,
            generation,
            handler,
            webview: Cell::new(std::ptr::null()),
        });
        // SAFETY: The NSObject subclass initializes its ivars before calling its superclass initializer.
        unsafe { msg_send![super(allocated), init] }
    }
    pub fn set_webview(&self, view: &WKWebView) {
        self.ivars().webview.set(view);
    }
    pub fn invalidate(&self) {
        self.ivars().webview.set(std::ptr::null());
    }
    fn fail(&self) {
        if let Some(owner) = self.ivars().owner.upgrade() {
            NativeWindow(owner).fail();
        }
    }
}

pub(super) fn initialization_script(generation: u64, policy: &BridgePolicy) -> String {
    let entry =
        serde_json::to_string(policy.entry.as_str()).expect("a URL string is JSON serializable");
    format!(
        r#"(() => {{
      const expected = new URL({entry});
      if (window !== window.top || location.protocol !== expected.protocol || location.host !== expected.host || location.pathname !== expected.pathname) return;
      const pending = new Map(), listeners = new Map(); let sequence = 0, closed = false;
      Object.defineProperty(window, '__ANTIBURN_WINDOW_GENERATION__', {{value:{generation}}});
      Object.defineProperty(window, '__ANTIBURN_NATIVE_DELIVER__', {{value:(message) => {{
        if (closed) return;
        if (message.event) {{ for (const callback of listeners.get(message.event) ?? []) callback(message.payload); return; }}
        const call = pending.get(message.id); if (!call) return;
        clearTimeout(call.timer); pending.delete(message.id);
        if (message.error !== undefined) call.reject(new Error(message.error)); else call.resolve(message.value);
      }}}});
      Object.defineProperty(window, '__ANTIBURN_NATIVE_PEEK__', {{value:Object.freeze({{
        invoke(command, args = {{}}) {{ return new Promise((resolve, reject) => {{
          if (closed || pending.size >= 128) {{ reject(new Error('Native preview is unavailable')); return; }}
          const id = ++sequence;
          const timer = setTimeout(() => {{ pending.delete(id); reject(new Error('Native preview request timed out')); }}, 10000);
          pending.set(id, {{resolve,reject,timer}});
          try {{ window.webkit.messageHandlers.anchored.postMessage(JSON.stringify({{id,rendererGeneration:{generation},command,args}})); }}
          catch (error) {{ clearTimeout(timer); pending.delete(id); reject(error); }}
        }}); }},
        async listen(event, callback) {{
          if (closed) throw new Error('Native preview is unavailable');
          if (!listeners.has(event)) listeners.set(event,new Set());
          listeners.get(event).add(callback);
          return () => listeners.get(event)?.delete(callback);
        }}
      }})}});
      addEventListener('pagehide', () => {{ closed = true; for (const call of pending.values()) {{clearTimeout(call.timer);call.reject(new Error('Native preview closed'));}} pending.clear(); listeners.clear(); }}, {{once:true}});
    }})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_policy_rejects_other_origins_and_documents() {
        let policy = BridgePolicy {
            entry: url::Url::parse("antiburn-anchored://localhost/native-peek.html").unwrap(),
        };
        assert!(policy.allows_document("antiburn-anchored://localhost/native-peek.html"));
        for url in [
            "antiburn-anchored://other/native-peek.html",
            "antiburn-anchored://localhost/index.html",
            "https://localhost/native-peek.html",
            "antiburn-anchored://user@localhost/native-peek.html",
        ] {
            assert!(!policy.allows_document(url));
        }
        let dev = BridgePolicy {
            entry: url::Url::parse("http://127.0.0.1:1420/native-peek.html").unwrap(),
        };
        assert!(dev.allows_document("http://127.0.0.1:1420/native-peek.html"));
        assert!(!dev.allows_document("http://127.0.0.1:1421/native-peek.html"));
    }
    #[test]
    fn asset_paths_are_confined_to_the_preview_and_built_assets() {
        assert_eq!(
            asset_path("antiburn-anchored://localhost/assets/example.js"),
            Some("assets/example.js")
        );
        for path in [
            "assets/../secret",
            "assets/%2e%2e/secret",
            "index.html",
            "assets//secret",
            "assets/a\\b",
            "assets/file.js?other",
        ] {
            assert!(asset_path(&format!("antiburn-anchored://localhost/{path}")).is_none());
        }
    }
    #[test]
    fn requests_require_current_renderer_and_allowlisted_operations() {
        let body = |generation, command| {
            serde_json::json!({"id":1,"rendererGeneration":generation,"command":command,"args":{}})
                .to_string()
        };
        assert!(Request::parse(&body(4, "get_popover_peek_state"), 4).is_some());
        assert!(Request::parse(&body(3, "popover_peek_ready"), 4).is_none());
        assert!(Request::parse(&body(4, "open_main"), 4).is_none());
        assert!(Request::parse(&"x".repeat(16_385), 4).is_none());
    }
}
