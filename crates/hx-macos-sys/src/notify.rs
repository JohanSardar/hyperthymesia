//! Desktop notifications through UNUserNotificationCenter, in this process so a
//! click reaches the app that owns the window. Only works inside the .app.
use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, class, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObject},
};
use objc2_foundation::{NSDictionary, NSError, NSObjectProtocol, NSString};
use std::sync::{Arc, Mutex, OnceLock};

#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

type Clicks = Arc<Mutex<Option<String>>>;
define_class!(
    #[unsafe(super = NSObject)]
    #[ivars = Clicks]
    struct NotificationDelegate;
    unsafe impl NSObjectProtocol for NotificationDelegate {}
    impl NotificationDelegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &AnyObject, response: &AnyObject, done: &block2::DynBlock<dyn Fn()>) {
            let target: Option<Retained<AnyObject>> = unsafe {
                let notification: Retained<AnyObject> = msg_send![response, notification];
                let request: Retained<AnyObject> = msg_send![&notification, request];
                let content: Retained<AnyObject> = msg_send![&request, content];
                let info: Retained<NSDictionary<NSString, AnyObject>> = msg_send![&content, userInfo];
                info.objectForKey(&NSString::from_str("target"))
            };
            if let Some(target) = target.and_then(|t| t.downcast::<NSString>().ok()) {
                *self.ivars().lock().expect("notification clicks") = Some(target.to_string());
            }
            done.call(());
        }
        /// Show the banner even while Hyperthymesia is the active app.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(&self, _center: &AnyObject, _notification: &AnyObject, done: &block2::DynBlock<dyn Fn(usize)>) {
            // banner | list | sound
            done.call((16 | 8 | 2,));
        }
    }
);

struct Center {
    center: Retained<AnyObject>,
    _delegate: Retained<NotificationDelegate>,
    clicks: Clicks,
}
// The center is documented thread-safe; the delegate only touches its Mutex.
unsafe impl Send for Center {}
unsafe impl Sync for Center {}

fn center() -> Option<&'static Center> {
    static CENTER: OnceLock<Option<Center>> = OnceLock::new();
    CENTER
        .get_or_init(|| {
            // Outside an app bundle UNUserNotificationCenter raises.
            let bundle = objc2_foundation::NSBundle::mainBundle();
            if bundle.bundleIdentifier().is_none()
                || !bundle.bundlePath().to_string().ends_with(".app")
            {
                return None;
            }
            let clicks: Clicks = Arc::default();
            let delegate: Retained<NotificationDelegate> = unsafe {
                msg_send![
                    super(NotificationDelegate::alloc().set_ivars(clicks.clone())),
                    init
                ]
            };
            let center: Retained<AnyObject> =
                unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] };
            let _: () = unsafe { msg_send![&center, setDelegate: &*delegate] };
            Some(Center {
                center,
                _delegate: delegate,
                clicks,
            })
        })
        .as_ref()
}

/// Call once at launch so a click that started the app is not missed.
pub fn notifications_init() {
    let _ = center();
}

/// Posts a notification. `target` comes back from `take_notification_click`.
/// macOS asks the person once whether Hyperthymesia may notify them.
pub fn notify(id: &str, title: &str, body: &str, target: &str) {
    let Some(center) = center() else { return };
    let content: Retained<AnyObject> =
        unsafe { msg_send![class!(UNMutableNotificationContent), new] };
    let info = NSDictionary::<NSString, AnyObject>::from_slices(
        &[&*NSString::from_str("target")],
        &[&*Retained::into_super(Retained::into_super(
            NSString::from_str(target),
        ))],
    );
    let request: Retained<AnyObject> = unsafe {
        let _: () = msg_send![&content, setTitle: &*NSString::from_str(title)];
        let _: () = msg_send![&content, setBody: &*NSString::from_str(body)];
        let _: () = msg_send![&content, setUserInfo: &*info];
        msg_send![class!(UNNotificationRequest), requestWithIdentifier: &*NSString::from_str(id),
            content: &*content, trigger: Option::<&AnyObject>::None]
    };
    let target = center.center.clone();
    let request = Mutex::new(Some(request));
    // alert | sound
    let granted = RcBlock::new(move |ok: Bool, _error: *mut NSError| {
        if let (true, Some(request)) = (ok.as_bool(), request.lock().expect("request").take()) {
            let _: () = unsafe {
                msg_send![&target, addNotificationRequest: &*request,
                    withCompletionHandler: Option::<&block2::DynBlock<dyn Fn(*mut NSError)>>::None]
            };
        }
    });
    let _: () = unsafe {
        msg_send![&center.center, requestAuthorizationWithOptions: 4usize | 2,
            completionHandler: &*granted]
    };
}

pub fn take_notification_click() -> Option<String> {
    center()?.clicks.lock().expect("notification clicks").take()
}
