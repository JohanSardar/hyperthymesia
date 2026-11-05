//! ServiceManagement calls never request authorization or open System Settings.
use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSError, NSString};
#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

pub fn register_launch_agent(plist: &str) -> Result<bool, String> {
    unsafe {
        let service: Retained<AnyObject> =
            msg_send![class!(SMAppService), agentServiceWithPlistName: &*NSString::from_str(plist)];
        let status: isize = msg_send![&*service, status];
        match status {
            1 => return Ok(true),
            2 => return Err("Login startup is disabled. Enable Hyperthymesia in System Settings > General > Login Items.".into()),
            _ => {}
        }
        let mut error: Option<Retained<NSError>> = None;
        let registered: bool = msg_send![&*service, registerAndReturnError: &mut error];
        if !registered {
            if error.as_ref().is_some_and(|error| error.code() == 3) {
                return Ok(false); // kSMErrorInvalidSignature
            }
            return Err(format!("Could not enable login startup: {error:?}"));
        }
        let status: isize = msg_send![&*service, status];
        if status != 1 {
            return Err(
                "Login startup needs approval in System Settings > General > Login Items.".into(),
            );
        }
        Ok(true)
    }
}
