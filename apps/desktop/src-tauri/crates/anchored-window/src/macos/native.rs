use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use super::bridge::{BridgePolicy, Delegate};
use crate::AnchoredWindowConfig;
use dispatch2::MainThreadBound;
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSPanel, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration};
use serde::Serialize;

pub type NativeRequestHandler =
    fn(&tauri::AppHandle, &str, serde_json::Value) -> Result<serde_json::Value, String>;

define_class!(
    #[unsafe(super(NSPanel, NSWindow))]
    #[thread_kind = MainThreadOnly]
    pub(crate) struct PassivePanel;
    unsafe impl NSObjectProtocol for PassivePanel {}
    impl PassivePanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

struct NativeConfiguration {
    policy: BridgePolicy,
    frame: NSRect,
    radius: f64,
    generation: u64,
    title: String,
}

pub(super) struct NativeObjects {
    pub panel: Retained<PassivePanel>,
    pub webview: Retained<WKWebView>,
    delegate: Retained<Delegate>,
}

pub(super) struct NativeState {
    pub app: tauri::AppHandle,
    objects: Mutex<Option<MainThreadBound<NativeObjects>>>,
    pub alive: AtomicBool,
    visible: AtomicBool,
    pub failed: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub(crate) struct NativeWindow(pub(super) Arc<NativeState>);

fn native_error(message: impl Into<String>) -> tauri::Error {
    tauri::Error::Io(std::io::Error::other(message.into()))
}

impl NativeWindow {
    pub(crate) fn create(
        app: &tauri::AppHandle,
        config: &AnchoredWindowConfig,
        generation: u64,
        height: f64,
        handler: NativeRequestHandler,
        failed: impl Fn() + Send + Sync + 'static,
    ) -> tauri::Result<Self> {
        let policy = BridgePolicy::new(app, &config.route)?;
        let state = Arc::new(NativeState {
            app: app.clone(),
            objects: Mutex::new(None),
            alive: AtomicBool::new(true),
            visible: AtomicBool::new(false),
            failed: Arc::new(failed),
        });
        let window = Self(state);
        let pending = window.clone();
        let configuration = NativeConfiguration {
            policy,
            frame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(config.width, height)),
            radius: config.corner_radius,
            generation,
            title: config.title.clone(),
        };
        app.run_on_main_thread(move || {
            if !pending.0.alive.load(Ordering::Acquire) {
                return;
            }
            let mtm = MainThreadMarker::new()
                .expect("native companion creation requires the main thread");
            match NativeObjects::new(mtm, &pending, configuration, handler) {
                Ok(objects) => {
                    *pending
                        .0
                        .objects
                        .lock()
                        .expect("native WebKit objects mutex must not be poisoned") =
                        Some(MainThreadBound::new(objects, mtm));
                }
                Err(error) => {
                    tracing::error!(%error, "native companion creation failed");
                    pending.fail();
                }
            }
        })?;
        Ok(window)
    }

    pub(super) fn fail(&self) {
        if !self.0.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        let failed = self.0.failed.clone();
        tauri::async_runtime::spawn_blocking(move || failed());
    }

    pub(super) fn with_objects(
        &self,
        action: impl FnOnce(&NativeObjects) + Send + 'static,
    ) -> tauri::Result<()> {
        let window = self.clone();
        self.0.app.run_on_main_thread(move || {
            if !window.0.alive.load(Ordering::Acquire) {
                return;
            }
            let mtm =
                MainThreadMarker::new().expect("native companion access requires the main thread");
            let objects = window
                .0
                .objects
                .lock()
                .expect("native WebKit objects mutex must not be poisoned");
            if let Some(objects) = objects.as_ref() {
                action(objects.get(mtm));
            }
        })
    }

    pub(crate) fn show(&self) -> tauri::Result<()> {
        let state = self.0.clone();
        self.with_objects(move |objects| {
            objects.panel.orderFrontRegardless();
            state.visible.store(true, Ordering::Release);
        })
    }

    pub(crate) fn hide(&self) -> tauri::Result<()> {
        self.0.visible.store(false, Ordering::Release);
        self.with_objects(|objects| objects.panel.orderOut(None))
    }

    pub(crate) fn is_visible(&self) -> tauri::Result<bool> {
        Ok(self.0.alive.load(Ordering::Acquire) && self.0.visible.load(Ordering::Acquire))
    }

    pub(crate) fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> tauri::Result<()> {
        let event = serde_json::to_string(event)?;
        let payload = serde_json::to_string(&payload)?;
        self.eval(format!(
            "window.__ANTIBURN_NATIVE_DELIVER__?.({{event:{event},payload:{payload}}})"
        ))
    }

    pub(super) fn eval(&self, script: String) -> tauri::Result<()> {
        self.with_objects(move |objects| {
            // SAFETY: The webview is live, and this callback runs on the main thread.
            unsafe {
                objects
                    .webview
                    .evaluateJavaScript_completionHandler(&NSString::from_str(&script), None);
            }
        })
    }

    pub(crate) fn destroy(&self) -> tauri::Result<()> {
        self.0.alive.store(false, Ordering::Release);
        let window = self.clone();
        self.0
            .app
            .run_on_main_thread(move || window.destroy_on_main_thread())
    }

    pub(crate) fn destroy_on_main_thread(&self) {
        let mtm =
            MainThreadMarker::new().expect("native companion destruction requires the main thread");
        self.0.alive.store(false, Ordering::Release);
        self.0.visible.store(false, Ordering::Release);
        let objects = self
            .0
            .objects
            .lock()
            .expect("native WebKit objects mutex must not be poisoned")
            .take();
        if let Some(objects) = objects {
            let objects = objects.into_inner(mtm);
            // SAFETY: All WebKit objects belong to the current main thread.
            unsafe {
                objects.webview.stopLoading();
                objects.webview.setNavigationDelegate(None);
                let controller = objects.webview.configuration().userContentController();
                controller.removeAllScriptMessageHandlers();
                controller.removeAllUserScripts();
            }
            objects.delegate.invalidate();
            objects.panel.orderOut(None);
            objects.panel.setContentView(None);
            objects.panel.close();
        }
    }
}

impl NativeObjects {
    fn new(
        mtm: MainThreadMarker,
        owner: &NativeWindow,
        configuration: NativeConfiguration,
        handler: NativeRequestHandler,
    ) -> tauri::Result<Self> {
        let NativeConfiguration {
            policy,
            frame,
            radius,
            generation,
            title,
        } = configuration;
        // SAFETY: The panel subclass has no ivars, and all objects are created on the main thread.
        unsafe {
            let panel: Retained<PassivePanel> = msg_send![PassivePanel::alloc(mtm), initWithContentRect: frame, styleMask: NSWindowStyleMask::NonactivatingPanel, backing: NSBackingStoreType::Buffered, defer: false];
            panel.setReleasedWhenClosed(false);
            panel.setTitle(&NSString::from_str(&title));
            panel.setHidesOnDeactivate(false);
            panel.setFloatingPanel(true);
            panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            let content = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
            content.setMaterial(NSVisualEffectMaterial::Popover);
            content.setState(NSVisualEffectState::Active);
            content.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            content.setWantsLayer(true);
            if let Some(layer) = content.layer() {
                layer.setCornerRadius(radius);
                layer.setMasksToBounds(true);
            }
            panel.setContentView(Some(&content));

            let configuration = WKWebViewConfiguration::new(mtm);
            let delegate = Delegate::new(
                mtm,
                Arc::downgrade(&owner.0),
                policy.clone(),
                generation,
                handler,
            );
            configuration.setURLSchemeHandler_forURLScheme(
                Some(ProtocolObject::from_ref(&*delegate)),
                &NSString::from_str(super::bridge::SCHEME),
            );
            let controller = configuration.userContentController();
            controller.addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*delegate),
                &NSString::from_str("anchored"),
            );
            let script = super::bridge::initialization_script(generation, &policy);
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(&script),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            );
            controller.addUserScript(&script);
            let webview = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                frame,
                &configuration,
            );
            delegate.set_webview(&webview);
            webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            webview.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            webview.setUnderPageBackgroundColor(Some(&NSColor::clearColor()));
            // WebKit exposes this background setting through its Objective-C property.
            let _: () = msg_send![&webview, setValue: &*objc2_foundation::NSNumber::numberWithBool(false), forKey: &*NSString::from_str("drawsBackground")];
            content.addSubview(&webview);
            let url = NSURL::URLWithString(&NSString::from_str(policy.entry.as_str()))
                .ok_or_else(|| native_error("invalid native companion URL"))?;
            webview.loadRequest(&NSURLRequest::requestWithURL(&url));
            Ok(Self {
                panel,
                webview,
                delegate,
            })
        }
    }
}
