use super::*;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem};
use objc2_foundation::NSObject;
struct Actions {
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Actions]
    struct MenuActions;
    unsafe impl NSObjectProtocol for MenuActions {}
    impl MenuActions {
        #[unsafe(method(toggle:))]
        fn toggle(&self,_sender:&objc2::runtime::AnyObject) {self.ivars().paused.fetch_xor(true,Ordering::Relaxed);}
        #[unsafe(method(quit:))]
        fn quit(&self,_sender:&objc2::runtime::AnyObject) {self.ivars().stop.store(true,Ordering::Relaxed);}
    }
);
pub struct MenuBar {
    status: Retained<NSStatusItem>,
    toggle: Retained<NSMenuItem>,
    actions: Retained<MenuActions>,
}
impl MenuBar {
    pub fn new(paused: Arc<AtomicBool>, stop: Arc<AtomicBool>) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let actions: Retained<MenuActions> = unsafe {
            let obj = MenuActions::alloc(mtm).set_ivars(Actions { paused, stop });
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
        menu.addItem(&toggle);
        menu.addItem(&quit);
        status.setMenu(Some(&menu));
        let mut this = Self {
            status,
            toggle,
            actions,
        };
        this.update();
        Some(this)
    }
    pub fn update(&mut self) {
        let paused = self.actions.ivars().paused.load(Ordering::Relaxed);
        if let Some(button) = self.status.button(self.actions.mtm()) {
            button.setTitle(&NSString::from_str(if paused {
                "Hx ◌"
            } else {
                "Hx ●"
            }));
        }
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
