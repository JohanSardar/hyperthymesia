//! The web app stays in the recorder's AppKit process, including its Dock identity.
use super::*;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{
    NSDictionary, NSError, NSHTTPCookie, NSObject, NSPoint, NSRect, NSSize, NSURL, NSURLRequest,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationDelegate, WKUIDelegate, WKWebView,
    WKWebViewConfiguration, WKWindowFeatures,
};
use std::cell::RefCell;

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = RefCell<String>]
    struct WebDelegate;
    unsafe impl NSObjectProtocol for WebDelegate {}
    unsafe impl WKNavigationDelegate for WebDelegate {
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        unsafe fn failed(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.show_error(web, error);
        }
        #[unsafe(method(webView:didFailNavigation:withError:))]
        unsafe fn failed_after_commit(&self, web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.show_error(web, error);
        }
    }
    unsafe impl WKUIDelegate for WebDelegate {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        unsafe fn new_window(&self, web: &WKWebView, _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction, _features: &WKWindowFeatures) -> Option<Retained<WKWebView>> {
            // Links with target=_blank stay in this app rather than disappearing.
            unsafe { web.loadRequest(&action.request()); }
            None
        }
    }
);
impl WebDelegate {
    fn show_error(&self, web: &WKWebView, error: &NSError) {
        if error.code() == objc2_foundation::NSURLErrorCancelled {
            return;
        }
        let base = NSURL::URLWithString(&NSString::from_str(&self.ivars().borrow()));
        let html = NSString::from_str(
            r#"<!doctype html><meta name="viewport" content="width=device-width">
<style>:root{color-scheme:light dark}body{font:15px -apple-system,sans-serif;max-width:440px;margin:18vh auto;padding:24px}h1{font-size:24px}a{color:inherit}</style>
<h1>Hyperthymesia couldn't connect</h1><p>Make sure your Hyperthymesia server is running, then try again.</p><a href="/app">Try again</a>"#,
        );
        unsafe {
            web.loadHTMLString_baseURL(&html, base.as_deref());
        }
    }
}

pub(crate) struct AppWindow {
    window: Retained<NSWindow>,
    web: Retained<WKWebView>,
    url: String,
    token: Option<String>,
    delegate: Retained<WebDelegate>,
}
impl AppWindow {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1120., 760.));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Miniaturizable
                    | NSWindowStyleMask::Resizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("Hyperthymesia"));
        unsafe { window.setReleasedWhenClosed(false) };
        window.setMinSize(NSSize::new(760., 540.));
        let name = NSString::from_str("HyperthymesiaAppWindow");
        if !window.setFrameUsingName(&name) {
            window.center();
        }
        window.setFrameAutosaveName(&name);
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        let web = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
        };
        let delegate: Retained<WebDelegate> = unsafe {
            msg_send![
                super(WebDelegate::alloc(mtm).set_ivars(RefCell::new(String::new()))),
                init
            ]
        };
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            web.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        window.setContentView(Some(&web));
        Self {
            window,
            web,
            url: String::new(),
            token: None,
            delegate,
        }
    }
    pub fn show(
        &mut self,
        url: &str,
        token: Option<&str>,
        mtm: MainThreadMarker,
    ) -> Result<(), String> {
        let target = NSURL::URLWithString(&NSString::from_str(url)).ok_or("Invalid app URL")?;
        let at_login = unsafe { self.web.URL() }
            .and_then(|url| url.path())
            .is_some_and(|path| path.to_string() == "/login");
        if self.url != url || self.token.as_deref() != token || (token.is_some() && at_login) {
            *self.delegate.ivars().borrow_mut() = url.to_owned();
            let request = NSURLRequest::requestWithURL(&target);
            if let Some(token) = token {
                let name = NSString::from_str("Set-Cookie");
                // The native app reseeds this session on launch. A fixed expiry
                // would send a still-open window back to the token-entry page.
                let header = NSString::from_str(&format!(
                    "hx_token={token}; HttpOnly; SameSite=Strict; Path=/"
                ));
                let headers = NSDictionary::from_slices(&[&*name], &[&*header]);
                let cookies =
                    NSHTTPCookie::cookiesWithResponseHeaderFields_forURL(&headers, &target);
                let cookie = cookies
                    .firstObject()
                    .ok_or("Could not establish app session")?;
                let web = self.web.clone();
                let ready = RcBlock::new(move || unsafe {
                    web.loadRequest(&request);
                });
                unsafe {
                    self.web
                        .configuration()
                        .websiteDataStore()
                        .httpCookieStore()
                        .setCookie_completionHandler(&cookie, Some(&ready));
                }
            } else {
                unsafe {
                    self.web.loadRequest(&request);
                }
            }
            self.url = url.to_owned();
            self.token = token.map(str::to_owned);
        }
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Regular);
        if self.window.isMiniaturized() {
            self.window.deminiaturize(None);
        }
        self.window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        Ok(())
    }
}

/// A manual launcher can hand focus to an already running login-managed app.
pub fn request_app_window() {
    unsafe {
        objc2_foundation::NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object_userInfo_deliverImmediately(
                &NSString::from_str("com.hyperthymesia.openApp"),
                None,
                None,
                true,
            );
    }
}
