use super::*;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplicationDelegate, NSApplicationTerminateReply, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem,
};
use objc2_foundation::NSObject;
struct Actions {
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,
    pair_requested: AtomicBool,
    app_requested: AtomicBool,
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
            self.ivars().app_requested.store(true, Ordering::Relaxed);
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
        #[unsafe(method(grantPermissions:))]
        fn grant_permissions(&self, _sender: &objc2::runtime::AnyObject) { onboard(); }
        #[unsafe(method(openApp:))]
        fn open_app(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().app_requested.store(true, Ordering::Relaxed); }
        #[unsafe(method(pair:))]
        fn pair(&self, _sender: &objc2::runtime::AnyObject) { self.ivars().pair_requested.store(true, Ordering::Relaxed); }
        #[unsafe(method(toggle:))]
        fn toggle(&self,_sender:&objc2::runtime::AnyObject) {self.ivars().paused.fetch_xor(true,Ordering::Relaxed);}
        #[unsafe(method(quit:))]
        fn quit(&self,_sender:&objc2::runtime::AnyObject) {self.ivars().quit_requested.store(true,Ordering::Relaxed);self.ivars().stop.store(true,Ordering::Relaxed);}
    }
);
pub struct MenuBar {
    status: Retained<NSStatusItem>,
    toggle: Retained<NSMenuItem>,
    health: Retained<NSMenuItem>,
    actions: Retained<MenuActions>,
    pairing: Retained<NSMenuItem>,
    permissions: Retained<NSMenuItem>,
    pair: Retained<NSMenuItem>,
    paired: bool,
    window: Option<super::app_window::AppWindow>,
}
impl MenuBar {
    pub fn new(
        paused: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        quit_requested: Arc<AtomicBool>,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let actions: Retained<MenuActions> = unsafe {
            let obj = MenuActions::alloc(mtm).set_ivars(Actions {
                paused,
                stop,
                quit_requested,
                pair_requested: AtomicBool::new(false),
                app_requested: AtomicBool::new(true),
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
        let toggle = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Pause capture"),
                Some(sel!(toggle:)),
                &NSString::from_str(""),
            )
        };
        let quit = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Quit"),
                Some(sel!(quit:)),
                &NSString::from_str(""),
            )
        };
        unsafe {
            toggle.setTarget(Some(&actions));
            quit.setTarget(Some(&actions));
        }
        let permissions = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Grant permissions"),
                Some(sel!(grantPermissions:)),
                &NSString::from_str(""),
            )
        };
        let pair = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Pair from clipboard"),
                Some(sel!(pair:)),
                &NSString::from_str(""),
            )
        };
        unsafe {
            permissions.setTarget(Some(&actions));
            pair.setTarget(Some(&actions));
        }
        let pairing = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Not paired"),
                None,
                &NSString::from_str(""),
            )
        };
        pairing.setEnabled(false);
        let health = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Screenshots are starting"),
                None,
                &NSString::from_str(""),
            )
        };
        health.setEnabled(false);
        let app = unsafe {
            let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str("Open Hyperthymesia"),
                Some(sel!(openApp:)),
                &NSString::from_str(""),
            );
            item.setTarget(Some(&actions));
            item
        };
        menu.addItem(&app);
        menu.addItem(&pairing);
        menu.addItem(&health);
        menu.addItem(&toggle);
        menu.addItem(&permissions);
        menu.addItem(&pair);
        menu.addItem(&quit);
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
            toggle,
            health,
            actions,
            pairing,
            permissions,
            pair,
            paired: false,
            window: None,
        };
        this.update(None);
        Some(this)
    }
    pub fn open_app(&mut self, url: &str) -> Result<(), String> {
        let mtm = self.actions.mtm();
        self.window
            .get_or_insert_with(|| super::app_window::AppWindow::new(mtm))
            .show(url, mtm)
    }
    pub fn take_open_app(&self) -> bool {
        self.actions
            .ivars()
            .app_requested
            .swap(false, Ordering::Relaxed)
    }
    pub fn set_paired(&mut self, paired: bool) {
        self.paired = paired;
        self.pairing.setHidden(paired);
        // Replacing a pairing redirects the outbox, so it stays a deliberate CLI step.
        self.pair.setHidden(paired);
    }
    /// Harnesses can display onboarding without ever requesting privacy access.
    pub fn disable_permissions(&mut self) {
        self.permissions.setEnabled(false);
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
    pub fn update(&mut self, warning: Option<&str>) {
        let paused = self.actions.ivars().paused.load(Ordering::Relaxed);
        if let Some(button) = self.status.button(self.actions.mtm()) {
            button.setTitle(&NSString::from_str(if paused {
                "Hx ◌"
            } else if !self.paired || warning.is_some() {
                "Hx !"
            } else {
                "Hx ●"
            }));
        }
        self.health.setTitle(&NSString::from_str(if paused {
            "Screenshots are paused"
        } else {
            warning.unwrap_or("Screenshots are ready")
        }));
        self.toggle.setTitle(&NSString::from_str(if paused {
            "Resume capture"
        } else {
            "Pause capture"
        }));
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
