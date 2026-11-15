use super::*;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplicationDelegate, NSApplicationTerminateReply, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem,
};
use objc2_foundation::NSObject;

/// What the person chose in the menu; the recorder applies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRequest {
    Pause15Minutes,
    PauseHour,
    PauseUntilTomorrow,
    Resume,
    /// Add or remove the app shown in the menu from never-record.
    ToggleNeverRecord,
    /// Start the setup guide for the permissions that are off.
    FixPermissions,
}

/// The recording line at the top of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recording {
    On,
    /// Shown as "Paused until {label}".
    Paused(String),
    /// Switched off in the app's Settings.
    Off,
}

struct Actions {
    stop: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,
    pair_requested: AtomicBool,
    /// A page of the web app to open; empty is its home page.
    open: Mutex<Option<String>>,
    request: Mutex<Option<MenuRequest>>,
}
impl Actions {
    fn ask(&self, request: MenuRequest) {
        *self.request.lock().expect("menu request") = Some(request);
    }
    fn open(&self, page: &str) {
        *self.open.lock().expect("menu open") = Some(page.into());
    }
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Actions]
    struct MenuActions;
    unsafe impl NSObjectProtocol for MenuActions {}
    unsafe impl NSApplicationDelegate for MenuActions {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            self.ivars().open("");
            false
        }
        #[unsafe(method(applicationShouldTerminate:))]
        fn terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            self.ivars().quit_requested.store(true, Ordering::Relaxed);
            self.ivars().stop.store(true, Ordering::Relaxed);
            NSApplicationTerminateReply::TerminateCancel
        }
    }
    impl MenuActions {
        #[unsafe(method(fixPermissions:))]
        fn fix_permissions(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().ask(MenuRequest::FixPermissions); }
        #[unsafe(method(openApp:))]
        fn open_app(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().open(""); }
        #[unsafe(method(openToday:))]
        fn open_today(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().open("today"); }
        #[unsafe(method(pair:))]
        fn pair(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().pair_requested.store(true, Ordering::Relaxed); }
        #[unsafe(method(pause15:))]
        fn pause15(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().ask(MenuRequest::Pause15Minutes); }
        #[unsafe(method(pauseHour:))]
        fn pause_hour(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().ask(MenuRequest::PauseHour); }
        #[unsafe(method(pauseTomorrow:))]
        fn pause_tomorrow(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().ask(MenuRequest::PauseUntilTomorrow); }
        #[unsafe(method(resume:))]
        fn resume(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().ask(MenuRequest::Resume); }
        #[unsafe(method(neverRecord:))]
        fn never_record(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().ask(MenuRequest::ToggleNeverRecord); }
        #[unsafe(method(quit:))]
        fn quit(&self,_sender:&objc2::runtime::AnyObject) {self.ivars().quit_requested.store(true,Ordering::Relaxed);self.ivars().stop.store(true,Ordering::Relaxed);}
    }
);

#[derive(Clone, PartialEq)]
struct Shown {
    recording: Recording,
    warning: Option<String>,
    missing: Vec<&'static str>,
    app: Option<(String, bool)>,
    paired: bool,
    fixable: bool,
}

pub struct MenuBar {
    status: Retained<NSStatusItem>,
    actions: Retained<MenuActions>,
    state_line: Retained<NSMenuItem>,
    warning: Retained<NSMenuItem>,
    permissions: Retained<NSMenuItem>,
    pause_items: Vec<Retained<NSMenuItem>>,
    resume: Retained<NSMenuItem>,
    never: Retained<NSMenuItem>,
    pairing: Retained<NSMenuItem>,
    pair: Retained<NSMenuItem>,
    recording: Recording,
    missing: Vec<&'static str>,
    app: Option<(String, bool)>,
    paired: bool,
    fixable: bool,
    shown: Option<Shown>,
    window: Option<super::app_window::AppWindow>,
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
    target: &MenuActions,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(""),
        )
    };
    if action.is_some() {
        unsafe { item.setTarget(Some(target)) };
    } else {
        item.setEnabled(false);
    }
    item
}
fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )?;
    image.setTemplate(true);
    Some(image)
}

impl MenuBar {
    pub fn new(stop: Arc<AtomicBool>, quit_requested: Arc<AtomicBool>) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let actions: Retained<MenuActions> = unsafe {
            let obj = MenuActions::alloc(mtm).set_ivars(Actions {
                stop,
                quit_requested,
                pair_requested: AtomicBool::new(false),
                open: Mutex::new(Some(String::new())),
                request: Mutex::new(None),
            });
            msg_send![super(obj), init]
        };
        unsafe {
            objc2_foundation::NSDistributedNotificationCenter::defaultCenter()
                .addObserver_selector_name_object_suspensionBehavior(
                    &actions,
                    sel!(openApp:),
                    Some(&NSString::from_str("com.hyperthymesia.openApp")),
                    None,
                    objc2_foundation::NSNotificationSuspensionBehavior::DeliverImmediately,
                );
        }
        let application = NSApplication::sharedApplication(mtm);
        application.setDelegate(Some(ProtocolObject::from_ref(&*actions)));
        let status = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let state_line = item(mtm, "Recording", None, &actions);
        let warning = item(mtm, "", None, &actions);
        let permissions = item(
            mtm,
            "Turn permissions back on…",
            Some(sel!(fixPermissions:)),
            &actions,
        );
        let pause_items = vec![
            item(mtm, "Pause for 15 minutes", Some(sel!(pause15:)), &actions),
            item(mtm, "Pause for 1 hour", Some(sel!(pauseHour:)), &actions),
            item(
                mtm,
                "Pause until tomorrow",
                Some(sel!(pauseTomorrow:)),
                &actions,
            ),
        ];
        let resume = item(mtm, "Resume recording", Some(sel!(resume:)), &actions);
        let never = item(
            mtm,
            "Never record this app",
            Some(sel!(neverRecord:)),
            &actions,
        );
        let today = item(mtm, "Open Today", Some(sel!(openToday:)), &actions);
        let app = item(mtm, "Open Hyperthymesia", Some(sel!(openApp:)), &actions);
        let pairing = item(mtm, "Not paired", None, &actions);
        let pair = item(mtm, "Pair from clipboard", Some(sel!(pair:)), &actions);
        let quit = item(mtm, "Quit Hyperthymesia", Some(sel!(quit:)), &actions);
        for entry in [&state_line, &warning, &permissions] {
            menu.addItem(entry);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for entry in pause_items.iter().chain([&resume, &never]) {
            menu.addItem(entry);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&today);
        menu.addItem(&app);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for entry in [&pairing, &pair, &quit] {
            menu.addItem(entry);
        }
        status.setMenu(Some(&menu));
        let main = NSMenu::new(mtm);
        for (title, entries) in [
            (
                "Hyperthymesia",
                vec![
                    ("About Hyperthymesia", "orderFrontStandardAboutPanel:", ""),
                    ("Hide Hyperthymesia", "hide:", "h"),
                    ("Quit Hyperthymesia", "quit:", "q"),
                ],
            ),
            (
                "Edit",
                vec![
                    ("Undo", "undo:", "z"),
                    ("Redo", "redo:", "Z"),
                    ("Cut", "cut:", "x"),
                    ("Copy", "copy:", "c"),
                    ("Paste", "paste:", "v"),
                    ("Select All", "selectAll:", "a"),
                ],
            ),
            (
                "Window",
                vec![
                    ("Open Hyperthymesia", "openApp:", "0"),
                    ("Minimize", "performMiniaturize:", "m"),
                    ("Zoom", "performZoom:", ""),
                    ("Close", "performClose:", "w"),
                ],
            ),
        ] {
            let root = NSMenuItem::new(mtm);
            root.setTitle(&NSString::from_str(title));
            let submenu = NSMenu::new(mtm);
            submenu.setTitle(&NSString::from_str(title));
            for (label, action, key) in entries {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        Some(objc2::runtime::Sel::register(
                            &std::ffi::CString::new(action).expect("static menu selector"),
                        )),
                        &NSString::from_str(key),
                    )
                };
                if action == "quit:" || action == "openApp:" {
                    unsafe {
                        item.setTarget(Some(&actions));
                    }
                }
                submenu.addItem(&item);
            }
            root.setSubmenu(Some(&submenu));
            main.addItem(&root);
            if title == "Window" {
                application.setWindowsMenu(Some(&submenu));
            }
        }
        application.setMainMenu(Some(&main));
        let mut this = Self {
            status,
            actions,
            state_line,
            warning,
            permissions,
            pause_items,
            resume,
            never,
            pairing,
            pair,
            recording: Recording::On,
            missing: vec![],
            app: None,
            paired: false,
            fixable: true,
            shown: None,
            window: None,
        };
        this.update(None);
        Some(this)
    }
    /// Opens a page of the web app; `page` is a path under `/app`, or empty.
    pub fn open_app(&mut self, url: &str, token: Option<&str>) -> Result<(), String> {
        let mtm = self.actions.mtm();
        self.window
            .get_or_insert_with(|| super::app_window::AppWindow::new(mtm))
            .show(url, token, mtm)
    }
    /// The web app page the person asked for, if any (empty: its home page).
    pub fn take_open_app(&self) -> Option<String> {
        self.actions.ivars().open.lock().expect("menu open").take()
    }
    /// Asks to open a page, for example after a notification was clicked.
    pub fn request_page(&self, page: &str) {
        self.actions.ivars().open(page);
    }
    pub fn take_request(&self) -> Option<MenuRequest> {
        self.actions
            .ivars()
            .request
            .lock()
            .expect("menu request")
            .take()
    }
    pub fn set_paired(&mut self, paired: bool) {
        self.paired = paired;
    }
    pub fn set_recording(&mut self, recording: Recording) {
        self.recording = recording;
    }
    /// Permissions that are off, by their System Settings names.
    pub fn set_missing(&mut self, missing: Vec<&'static str>) {
        self.missing = missing;
    }
    /// The app in front, and whether it is already never recorded.
    pub fn set_app(&mut self, app: Option<(String, bool)>) {
        self.app = app;
    }
    /// Harnesses can display onboarding without ever requesting privacy access.
    pub fn disable_permissions(&mut self) {
        self.fixable = false;
    }
    pub fn take_pair_link(&mut self) -> Option<Result<String, String>> {
        if !self
            .actions
            .ivars()
            .pair_requested
            .swap(false, Ordering::Relaxed)
        {
            return None;
        }
        let text = unsafe {
            objc2_app_kit::NSPasteboard::generalPasteboard()
                .stringForType(objc2_app_kit::NSPasteboardTypeString)
        };
        Some(
            text.map(|s| s.to_string())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| "Copy a pairing link, then choose Pair from clipboard.".into()),
        )
    }
    /// Redraws only when something changed; called on every loop turn.
    pub fn update(&mut self, warning: Option<&str>) {
        let next = Shown {
            recording: self.recording.clone(),
            warning: warning.map(str::to_owned),
            missing: self.missing.clone(),
            app: self.app.clone(),
            paired: self.paired,
            fixable: self.fixable,
        };
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        let attention = !next.missing.is_empty() || next.warning.is_some() || !next.paired;
        let (icon, line) = match &next.recording {
            Recording::Paused(until) => ("pause.circle", format!("Paused until {until}")),
            Recording::Off => ("circle.slash", "Recording is off in Settings".to_owned()),
            Recording::On if attention => (
                "exclamationmark.triangle",
                "Recording, with a problem".to_owned(),
            ),
            Recording::On => ("record.circle", "Recording".to_owned()),
        };
        if let Some(button) = self.status.button(self.actions.mtm()) {
            match symbol(icon, &line) {
                Some(image) => {
                    button.setImage(Some(&image));
                    button.setTitle(&NSString::from_str(""));
                }
                None => button.setTitle(&NSString::from_str("Hx")),
            }
            button.setToolTip(Some(&NSString::from_str(&format!("Hyperthymesia: {line}"))));
        }
        self.state_line.setTitle(&NSString::from_str(&line));
        self.warning.setHidden(next.warning.is_none());
        self.warning
            .setTitle(&NSString::from_str(next.warning.as_deref().unwrap_or("")));
        self.permissions.setHidden(next.missing.is_empty());
        self.permissions.setEnabled(next.fixable);
        self.permissions
            .setTitle(&NSString::from_str(&match next.missing.as_slice() {
                [one] => format!("{one} is off. Turn it back on…"),
                many => format!("{} are off. Turn them back on…", many.join(" and ")),
            }));
        let paused = matches!(next.recording, Recording::Paused(_));
        let off = next.recording == Recording::Off;
        for entry in &self.pause_items {
            entry.setHidden(paused || off);
        }
        self.resume.setHidden(!paused);
        match &next.app {
            Some((name, never)) => {
                self.never.setHidden(false);
                self.never.setTitle(&NSString::from_str(&if *never {
                    format!("Record {name} again")
                } else {
                    format!("Never record {name}")
                }));
            }
            None => self.never.setHidden(true),
        }
        self.pairing.setHidden(next.paired);
        // Replacing a pairing redirects the outbox, so it stays a deliberate CLI step.
        self.pair.setHidden(next.paired);
        self.shown = Some(next);
    }
}
impl Drop for MenuBar {
    fn drop(&mut self) {
        NSApplication::sharedApplication(self.actions.mtm()).setDelegate(None);
        unsafe {
            objc2_foundation::NSDistributedNotificationCenter::defaultCenter()
                .removeObserver(&self.actions);
        }
        NSStatusBar::systemStatusBar().removeStatusItem(&self.status);
    }
}
