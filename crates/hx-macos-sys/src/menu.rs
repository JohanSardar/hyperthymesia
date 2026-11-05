use super::*;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem};
use objc2_foundation::NSObject;
struct Actions {
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    quit_requested: Arc<AtomicBool>,
    pair_requested: AtomicBool,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Actions]
    struct MenuActions;
    unsafe impl NSObjectProtocol for MenuActions {}
    impl MenuActions {
        #[unsafe(method(grantPermissions:))]
        fn grant_permissions(&self, _sender: &objc2::runtime::AnyObject) { onboard(); }
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
            });
            msg_send![super(obj), init]
        };
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
                &NSString::from_str("Screen recording is starting"),
                None,
                &NSString::from_str(""),
            )
        };
        health.setEnabled(false);
        menu.addItem(&pairing);
        menu.addItem(&health);
        menu.addItem(&toggle);
        menu.addItem(&permissions);
        menu.addItem(&pair);
        menu.addItem(&quit);
        status.setMenu(Some(&menu));
        let mut this = Self {
            status,
            toggle,
            health,
            actions,
            pairing,
            permissions,
            pair,
            paired: false,
        };
        this.update(None);
        Some(this)
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
            "Screen recording is paused"
        } else {
            warning.unwrap_or("Screen recording is ready")
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
        NSStatusBar::systemStatusBar().removeStatusItem(&self.status);
    }
}
