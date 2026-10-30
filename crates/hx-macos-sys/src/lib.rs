//! Audited macOS boundary. No permission requests occur outside `onboard`.
use block2::RcBlock;
use crossbeam_queue::ArrayQueue;
use objc2::{MainThreadMarker, Message, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{NSApplication, NSWorkspace};
use objc2_application_services::{AXIsProcessTrusted, AXUIElement};
use objc2_core_foundation::{
    CFArray, CFRetained, CFRunLoop, CFString, CFType, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::*;
use objc2_foundation::{NSDate, NSNotificationCenter, NSObjectProtocol, NSString};
use serde::Serialize;
use std::{
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Permissions {
    pub accessibility: bool,
    pub screen: bool,
    pub input: bool,
}
pub fn permissions() -> Permissions {
    Permissions {
        accessibility: unsafe { AXIsProcessTrusted() },
        screen: CGPreflightScreenCaptureAccess(),
        input: CGPreflightListenEventAccess(),
    }
}
/// Explicit user initiated onboarding only.
pub fn onboard() -> Permissions {
    // AX's prompt option is the sole request path for accessibility.
    unsafe {
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let options = objc2_foundation::NSDictionary::from_slices(
            &[&*key],
            &[&*objc2_foundation::NSNumber::new_bool(true)],
        );
        unsafe extern "C" {
            fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
        }
        AXIsProcessTrustedWithOptions((&*options as *const _) as *const _);
    }
    CGRequestScreenCaptureAccess();
    CGRequestListenEventAccess();
    permissions()
}
#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}
unsafe extern "C" {
    fn mach_continuous_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> i32;
}
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}
pub fn secure_input() -> bool {
    unsafe { IsSecureEventInputEnabled() }
}
pub fn continuous_nanos() -> u64 {
    let mut tb = Timebase { numer: 0, denom: 0 };
    unsafe {
        mach_timebase_info(&mut tb);
        ((mach_continuous_time() as u128 * tb.numer as u128) / tb.denom.max(1) as u128) as u64
    }
}
pub fn boot_id() -> std::io::Result<String> {
    let mut time: libc::timeval = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of_val(&time);
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.boottime".as_ptr(),
            (&mut time as *mut libc::timeval).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(format!("{}:{}", time.tv_sec, time.tv_usec))
}
pub fn idle_seconds() -> f64 {
    CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        CGEventType(u32::MAX),
    )
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Focus {
    pub bundle_id: String,
    pub name: String,
    pub pid: i32,
    pub title: Option<String>,
    pub electron: bool,
}
pub fn frontmost() -> Option<Focus> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(focus_from_app(&app))
}
fn focus_from_app(app: &objc2_app_kit::NSRunningApplication) -> Focus {
    let path = app
        .bundleURL()
        .and_then(|u| u.path())
        .map(|s| s.to_string());
    Focus {
        bundle_id: app
            .bundleIdentifier()
            .map(|v| v.to_string())
            .unwrap_or_default(),
        name: app
            .localizedName()
            .map(|v| v.to_string())
            .unwrap_or_default(),
        pid: app.processIdentifier(),
        title: None,
        electron: path.is_some_and(|p| {
            std::path::Path::new(&p)
                .join("Contents/Frameworks/Electron Framework.framework")
                .exists()
        }),
    }
}
pub fn skip_ax(focus: &Focus) -> bool {
    focus.electron
        || [
            "com.google.Chrome",
            "com.microsoft.edgemac",
            "com.brave.Browser",
            "org.chromium.Chromium",
            "com.operasoftware.Opera",
            "company.thebrowser.Browser",
            "com.vivaldi.Vivaldi",
        ]
        .iter()
        .any(|b| focus.bundle_id.starts_with(b))
}
fn attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let mut value = std::ptr::null();
    // Owned CF value is returned only on success, checked before adoption.
    unsafe {
        element.set_messaging_timeout(0.025);
        element.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut value));
        NonNull::new(value.cast_mut()).map(|p| CFRetained::from_raw(p))
    }
}
fn string_attr(element: &AXUIElement, name: &str) -> Option<String> {
    attribute(element, name)?
        .downcast::<CFString>()
        .ok()
        .map(|s| s.to_string())
}
fn focused_window(pid: i32) -> Option<CFRetained<AXUIElement>> {
    if !permissions().accessibility {
        return None;
    }
    let app = unsafe { AXUIElement::new_application(pid) };
    attribute(&app, "AXFocusedWindow")?.downcast().ok()
}
pub fn window_title(pid: i32) -> Option<String> {
    string_attr(&*focused_window(pid)?, "AXTitle")
}
#[derive(Debug)]
pub struct Text {
    pub text: String,
    pub truncated: bool,
    pub nodes: usize,
}
pub fn window_text(
    focus: &Focus,
    node_cap: usize,
    deadline: Duration,
    byte_cap: usize,
) -> Option<Text> {
    if skip_ax(focus) || unverifiable_private_state(focus) || secure_input() {
        return None;
    }
    let start = Instant::now();
    let mut stack = vec![focused_window(focus.pid)?];
    let mut text = String::new();
    let mut nodes = 0;
    let mut truncated = false;
    while let Some(node) = stack.pop() {
        if nodes >= node_cap || start.elapsed() >= deadline || text.len() >= byte_cap {
            return Some(Text {
                text,
                truncated: true,
                nodes,
            });
        }
        nodes += 1;
        // Password controls are never read, even when Secure Input is not enabled.
        if string_attr(&node, "AXSubrole").as_deref() == Some("AXSecureTextField") {
            continue;
        }
        for key in ["AXTitle", "AXValue", "AXDescription"] {
            if start.elapsed() >= deadline {
                return Some(Text {
                    text,
                    truncated: true,
                    nodes,
                });
            }
            if let Some(value) = string_attr(&node, key) {
                for ch in value.chars() {
                    if text.len() + ch.len_utf8() >= byte_cap {
                        truncated = true;
                        break;
                    }
                    text.push(ch);
                }
                if text.len() < byte_cap {
                    text.push('\n');
                }
            }
        }
        let limit = node_cap.saturating_sub(nodes + stack.len());
        let mut count = 0isize;
        unsafe {
            node.attribute_value_count(
                &CFString::from_str("AXChildren"),
                NonNull::from(&mut count),
            );
        }
        if count > 0 {
            if count as usize > limit {
                truncated = true;
            }
            let mut raw = std::ptr::null();
            unsafe {
                node.copy_attribute_values(
                    &CFString::from_str("AXChildren"),
                    0,
                    (count as usize).min(limit) as isize,
                    NonNull::from(&mut raw),
                );
            }
            if let Some(raw) = NonNull::new(raw.cast_mut()) {
                let children = unsafe {
                    CFRetained::cast_unchecked::<CFArray<CFType>>(CFRetained::from_raw(raw))
                };
                for child in children.iter() {
                    if let Ok(child) = child.downcast::<AXUIElement>() {
                        stack.push(child);
                    }
                }
            }
        }
    }
    Some(Text {
        text,
        truncated,
        nodes,
    })
}
#[derive(Debug)]
pub struct Hit {
    pub role: Option<String>,
    pub label: Option<String>,
}
pub fn hit_test(x: f64, y: f64) -> Option<Hit> {
    if !permissions().accessibility || secure_input() {
        return None;
    }
    unsafe {
        let system = AXUIElement::new_system_wide();
        system.set_messaging_timeout(0.025);
        let mut raw = std::ptr::null();
        system.copy_element_at_position(x as f32, y as f32, NonNull::from(&mut raw));
        let element = CFRetained::from_raw(NonNull::new(raw.cast_mut())?);
        if string_attr(&element, "AXSubrole").as_deref() == Some("AXSecureTextField") {
            return None;
        }
        Some(Hit {
            role: string_attr(&element, "AXRole"),
            label: string_attr(&element, "AXTitle")
                .or_else(|| string_attr(&element, "AXDescription")),
        })
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Input {
    pub kind: u32,
    pub x: f64,
    pub y: f64,
    pub nanos: u64,
}
struct InputState {
    queue: ArrayQueue<Input>,
    dropped: AtomicU64,
    stop: AtomicBool,
    disabled: AtomicBool,
}
pub struct InputTap {
    state: Arc<InputState>,
    thread: Option<std::thread::JoinHandle<()>>,
}
unsafe extern "C-unwind" fn input_callback(
    _: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut std::ffi::c_void,
) -> *mut CGEvent {
    let state = unsafe { &*info.cast::<InputState>() };
    if kind.0 >= u32::MAX - 1 {
        state.disabled.store(true, Ordering::Relaxed);
        return event.as_ptr();
    }
    let p = CGEvent::location(Some(unsafe { event.as_ref() }));
    if state
        .queue
        .push(Input {
            kind: kind.0,
            x: p.x,
            y: p.y,
            nanos: continuous_nanos(),
        })
        .is_err()
    {
        state.dropped.fetch_add(1, Ordering::Relaxed);
    }
    event.as_ptr()
}
impl InputTap {
    pub fn start() -> Option<Self> {
        if !permissions().input {
            return None;
        }
        let state = Arc::new(InputState {
            queue: ArrayQueue::new(4096),
            dropped: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            disabled: AtomicBool::new(false),
        });
        let worker = state.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || unsafe {
            let mask = [1, 3, 5, 6, 7, 10, 22, 25, 26, 27]
                .into_iter()
                .fold(0, |m, k| m | (1u64 << k));
            let Some(tap) = CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                mask,
                Some(input_callback),
                Arc::as_ptr(&worker).cast_mut().cast(),
            ) else {
                let _ = tx.send(false);
                return;
            };
            let source =
                objc2_core_foundation::CFMachPort::new_run_loop_source(None, Some(&tap), 0)
                    .expect("event tap runloop source");
            let runloop = CFRunLoop::current().expect("thread runloop");
            runloop.add_source(Some(&source), kCFRunLoopDefaultMode);
            let _ = tx.send(true);
            while !worker.stop.load(Ordering::Relaxed) {
                CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, 0.1, false);
                if worker.disabled.swap(false, Ordering::Relaxed) {
                    CGEvent::tap_enable(&tap, true);
                }
            }
            CGEvent::tap_enable(&tap, false);
            runloop.remove_source(Some(&source), kCFRunLoopDefaultMode);
        });
        if !rx.recv().unwrap_or(false) {
            let _ = thread.join();
            return None;
        }
        Some(Self {
            state,
            thread: Some(thread),
        })
    }
    pub fn pop(&self) -> Option<Input> {
        self.state.queue.pop()
    }
    pub fn dropped(&self) -> u64 {
        self.state.dropped.load(Ordering::Relaxed)
    }
}
impl Drop for InputTap {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEvent {
    Activation,
    Sleep,
    Wake,
    DisplaySleep,
    DisplayWake,
    Lock,
    Unlock,
    Power,
    Thermal,
}
#[derive(Debug, Clone)]
pub struct NativeSignal {
    pub event: NativeEvent,
    pub nanos: u64,
    pub focus: Option<Focus>,
}
struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}
impl Drop for Observer {
    fn drop(&mut self) {
        unsafe {
            self.center.removeObserver((*self.token).as_ref());
        }
    }
}
pub struct MainLoop {
    _app: Retained<NSApplication>,
    observers: Vec<Observer>,
    events: Arc<ArrayQueue<NativeSignal>>,
    dropped: Arc<AtomicU64>,
    power_source: Option<CFRetained<objc2_core_foundation::CFRunLoopSource>>,
}
impl MainLoop {
    pub fn new() -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
        let mut this = Self {
            _app: app,
            observers: Vec::new(),
            events: Arc::new(ArrayQueue::new(256)),
            dropped: Arc::new(AtomicU64::new(0)),
            power_source: None,
        };
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        for (name, event) in [
            (
                "NSWorkspaceDidActivateApplicationNotification",
                NativeEvent::Activation,
            ),
            ("NSWorkspaceWillSleepNotification", NativeEvent::Sleep),
            ("NSWorkspaceDidWakeNotification", NativeEvent::Wake),
            (
                "NSWorkspaceScreensDidSleepNotification",
                NativeEvent::DisplaySleep,
            ),
            (
                "NSWorkspaceScreensDidWakeNotification",
                NativeEvent::DisplayWake,
            ),
        ] {
            this.observe(&center, name, event);
        }
        let distributed = objc2_foundation::NSDistributedNotificationCenter::defaultCenter();
        // NSDistributedNotificationCenter inherits NSNotificationCenter.
        this.observe(&distributed, "com.apple.screenIsLocked", NativeEvent::Lock);
        this.observe(
            &distributed,
            "com.apple.screenIsUnlocked",
            NativeEvent::Unlock,
        );
        this.observe(
            &NSNotificationCenter::defaultCenter(),
            "NSProcessInfoThermalStateDidChangeNotification",
            NativeEvent::Thermal,
        );
        this.observe(
            &NSNotificationCenter::defaultCenter(),
            "NSProcessInfoPowerStateDidChangeNotification",
            NativeEvent::Power,
        );
        #[link(name = "IOKit", kind = "framework")]
        unsafe extern "C" {
            fn IOPSNotificationCreateRunLoopSource(
                callback: unsafe extern "C" fn(*mut std::ffi::c_void),
                context: *mut std::ffi::c_void,
            ) -> *mut objc2_core_foundation::CFRunLoopSource;
        }
        unsafe extern "C" fn power_changed(context: *mut std::ffi::c_void) {
            let queue = unsafe { &*context.cast::<ArrayQueue<NativeSignal>>() };
            let _ = queue.push(NativeSignal {
                event: NativeEvent::Power,
                nanos: continuous_nanos(),
                focus: None,
            });
        }
        this.power_source = unsafe {
            NonNull::new(IOPSNotificationCreateRunLoopSource(
                power_changed,
                Arc::as_ptr(&this.events).cast_mut().cast(),
            ))
            .map(|p| CFRetained::from_raw(p))
        };
        if let Some(source) = &this.power_source {
            unsafe {
                CFRunLoop::current()?.add_source(Some(source), kCFRunLoopDefaultMode);
            }
        }
        Some(this)
    }
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    fn observe(&mut self, center: &NSNotificationCenter, name: &str, event: NativeEvent) {
        let queue = self.events.clone();
        let dropped = self.dropped.clone();
        let block = RcBlock::new(
            move |notification: NonNull<objc2_foundation::NSNotification>| {
                let nanos = continuous_nanos();
                let focus = if event == NativeEvent::Activation {
                    unsafe { notification.as_ref().userInfo() }
                        .and_then(|info| {
                            info.objectForKey(&NSString::from_str("NSWorkspaceApplicationKey"))
                        })
                        .and_then(|app| app.downcast::<objc2_app_kit::NSRunningApplication>().ok())
                        .map(|app| focus_from_app(&app))
                } else {
                    None
                };
                if queue
                    .push(NativeSignal {
                        event,
                        nanos,
                        focus,
                    })
                    .is_err()
                {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            },
        );
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(&NSString::from_str(name)),
                None,
                None,
                &block,
            )
        };
        self.observers.push(Observer {
            center: center.retain(),
            token,
        });
    }
    pub fn pump(&self, duration: Duration) {
        objc2::rc::autoreleasepool(|_| {
            let until = NSDate::dateWithTimeIntervalSinceNow(duration.as_secs_f64());
            if let Some(event) = self._app.nextEventMatchingMask_untilDate_inMode_dequeue(
                objc2_app_kit::NSEventMask::Any,
                Some(&until),
                unsafe { objc2_foundation::NSDefaultRunLoopMode },
                true,
            ) {
                self._app.sendEvent(&event);
            }
        });
    }
    pub fn pop(&self) -> Option<NativeSignal> {
        self.events.pop()
    }
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Power {
    pub battery: bool,
    pub thermal: i64,
}
pub fn power() -> Power {
    // IOKit returns a retained snapshot; no helper process or authorization involved.
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> *mut CFType;
        fn IOPSGetProvidingPowerSourceType(info: *const CFType) -> *const CFString;
    }
    let battery = unsafe {
        NonNull::new(IOPSCopyPowerSourcesInfo()).is_some_and(|p| {
            let info = CFRetained::from_raw(p);
            let source = IOPSGetProvidingPowerSourceType(&*info);
            !source.is_null() && (*source).to_string() == "Battery Power"
        })
    };
    Power {
        battery,
        thermal: objc2_foundation::NSProcessInfo::processInfo()
            .thermalState()
            .0 as i64,
    }
}

mod menu;
pub use menu::MenuBar;

pub struct AxObserver {
    observer: CFRetained<objc2_application_services::AXObserver>,
    source: CFRetained<objc2_core_foundation::CFRunLoopSource>,
    changed: Box<AtomicBool>,
    pub unsupported: Vec<i32>,
}
unsafe extern "C-unwind" fn ax_callback(
    _: NonNull<objc2_application_services::AXObserver>,
    _: NonNull<AXUIElement>,
    _: NonNull<CFString>,
    info: *mut std::ffi::c_void,
) {
    unsafe { &*info.cast::<AtomicBool>() }.store(true, Ordering::Relaxed);
}
impl AxObserver {
    pub fn new(focus: &Focus) -> Option<Self> {
        if !permissions().accessibility || skip_ax(focus) {
            return None;
        }
        unsafe {
            let app = AXUIElement::new_application(focus.pid);
            app.set_messaging_timeout(0.025);
            let mut raw = std::ptr::null_mut();
            objc2_application_services::AXObserver::create(
                focus.pid,
                Some(ax_callback),
                NonNull::from(&mut raw),
            );
            let observer = CFRetained::from_raw(NonNull::new(raw)?);
            let mut changed = Box::new(AtomicBool::new(false));
            let mut unsupported = vec![];
            for name in ["AXFocusedWindowChanged", "AXTitleChanged"] {
                let err = observer.add_notification(
                    &app,
                    &CFString::from_str(name),
                    (&mut *changed as *mut AtomicBool).cast(),
                );
                if err.0 != 0 {
                    unsupported.push(err.0);
                }
            }
            if let Some(window) = focused_window(focus.pid) {
                let err = observer.add_notification(
                    &window,
                    &CFString::from_str("AXTitleChanged"),
                    (&mut *changed as *mut AtomicBool).cast(),
                );
                if err.0 != 0 {
                    unsupported.push(err.0);
                }
            }
            let source = observer.run_loop_source();
            CFRunLoop::current()?.add_source(Some(&source), kCFRunLoopDefaultMode);
            Some(Self {
                observer,
                source,
                changed,
                unsupported,
            })
        }
    }
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }
}
impl Drop for AxObserver {
    fn drop(&mut self) {
        if let Some(runloop) = CFRunLoop::current() {
            unsafe {
                runloop.remove_source(Some(&self.source), kCFRunLoopDefaultMode);
            }
        }
        let _ = &self.observer;
    }
}
mod media;
pub use media::{Frame, ScreenStream, VideoWriter};

/// Browsers without a trusted private-window signal are metadata-only at the OS layer.
pub fn unverifiable_private_state(focus: &Focus) -> bool {
    [
        "com.apple.Safari",
        "org.mozilla.firefox",
        "com.google.Chrome",
        "com.microsoft.edgemac",
        "com.brave.Browser",
        "org.chromium.Chromium",
        "com.operasoftware.Opera",
        "company.thebrowser.Browser",
        "com.vivaldi.Vivaldi",
    ]
    .iter()
    .any(|b| focus.bundle_id.starts_with(b))
}
pub fn session_locked() -> Option<bool> {
    let dictionary = CGSessionCopyCurrentDictionary()?;
    let dictionary = unsafe {
        CFRetained::cast_unchecked::<objc2_core_foundation::CFDictionary<CFString, CFType>>(
            dictionary,
        )
    };
    dictionary
        .get(&CFString::from_str("CGSSessionScreenIsLocked"))?
        .downcast::<objc2_core_foundation::CFBoolean>()
        .ok()
        .map(|v| v.value())
}

impl Drop for MainLoop {
    fn drop(&mut self) {
        if let Some(source) = &self.power_source
            && let Some(runloop) = CFRunLoop::current()
        {
            unsafe {
                runloop.remove_source(Some(source), kCFRunLoopDefaultMode);
            }
        }
    }
}
pub fn display_asleep() -> bool {
    CGDisplayIsAsleep(CGMainDisplayID())
}
pub fn host_name() -> String {
    let mut buffer = [0u8; 256];
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return "mac-unknown".into();
    }
    let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuous_clock_and_boot_identity_are_stable() {
        let a = continuous_nanos();
        let b = continuous_nanos();
        assert!(b >= a);
        assert_eq!(boot_id().unwrap(), boot_id().unwrap());
    }
    #[test]
    fn chromium_and_electron_are_never_forced_into_ax() {
        let mut f = Focus {
            bundle_id: "com.google.Chrome.beta".into(),
            name: String::new(),
            pid: 0,
            title: None,
            electron: false,
        };
        assert!(skip_ax(&f));
        f.bundle_id = "native.app".into();
        assert!(!skip_ax(&f));
        f.electron = true;
        assert!(skip_ax(&f));
        f.bundle_id = "com.apple.Safari".into();
        assert!(unverifiable_private_state(&f));
    }
}
