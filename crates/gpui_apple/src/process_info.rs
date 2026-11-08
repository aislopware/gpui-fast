use block2::RcBlock;
use futures::{StreamExt, channel::mpsc};
use gpui::{ForegroundExecutor, PowerMode, Task, ThermalState};
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol, NSProcessInfo,
    NSProcessInfoPowerStateDidChangeNotification, NSProcessInfoThermalState,
    NSProcessInfoThermalStateDidChangeNotification,
};

pub fn thermal_state() -> ThermalState {
    match NSProcessInfo::processInfo().thermalState() {
        NSProcessInfoThermalState::Fair => ThermalState::Fair,
        NSProcessInfoThermalState::Serious => ThermalState::Serious,
        NSProcessInfoThermalState::Critical => ThermalState::Critical,
        _ => ThermalState::Nominal,
    }
}

pub fn power_mode() -> PowerMode {
    if NSProcessInfo::processInfo().isLowPowerModeEnabled() {
        PowerMode::LowPower
    } else {
        // This API does not distinguish Automatic from High Power Mode on macOS.
        PowerMode::Unknown
    }
}

pub fn observe_thermal_state(
    executor: &ForegroundExecutor,
    callback: Box<dyn FnMut()>,
) -> NotificationObserver {
    // Foundation requires reading thermalState before registering for changes.
    // https://developer.apple.com/documentation/foundation/processinfo/thermalstatedidchangenotification
    thermal_state();
    NotificationObserver::process_info(
        unsafe { NSProcessInfoThermalStateDidChangeNotification },
        executor,
        callback,
    )
}

pub fn observe_power_mode(
    executor: &ForegroundExecutor,
    callback: Box<dyn FnMut()>,
) -> NotificationObserver {
    NotificationObserver::process_info(
        unsafe { NSProcessInfoPowerStateDidChangeNotification },
        executor,
        callback,
    )
}

pub struct NotificationObserver {
    center: Retained<NSNotificationCenter>,
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    _task: Task<()>,
}

/// A notification observed on the foreground executor until dropped.
impl NotificationObserver {
    fn process_info(
        name: &NSNotificationName,
        executor: &ForegroundExecutor,
        callback: Box<dyn FnMut()>,
    ) -> Self {
        Self::new(
            NSNotificationCenter::defaultCenter(),
            name,
            Some(&NSProcessInfo::processInfo()),
            executor,
            callback,
        )
    }

    /// Calls `callback` on `executor` for every `name` that `center` posts from `object`
    /// (from any object when `None`).
    pub fn new(
        center: Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        object: Option<&AnyObject>,
        executor: &ForegroundExecutor,
        mut callback: Box<dyn FnMut()>,
    ) -> Self {
        let (sender, mut receiver) = mpsc::unbounded();
        let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
            // Notifications can arrive synchronously or off-thread. Defer application
            // callbacks so they cannot reenter GPUI while it is already borrowed.
            if sender.unbounded_send(()).is_err() {
                log::debug!("process info observer stopped before notification delivery");
            }
        });
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), object, None, &block)
        };
        let task = executor.spawn(async move {
            while receiver.next().await.is_some() {
                callback();
            }
        });
        Self {
            center,
            observer,
            _task: task,
        }
    }
}

impl Drop for NotificationObserver {
    fn drop(&mut self) {
        unsafe {
            self.center.removeObserver(self.observer.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    fn post_notification(center: &NSNotificationCenter, name: &NSNotificationName) {
        unsafe {
            center.postNotificationName_object(name, Some(&NSProcessInfo::processInfo()));
        }
    }

    #[gpui::test]
    fn notifications_are_deferred_and_cancelled_on_drop(cx: &mut gpui::TestAppContext) {
        for name in unsafe {
            [
                NSProcessInfoPowerStateDidChangeNotification,
                NSProcessInfoThermalStateDidChangeNotification,
            ]
        } {
            check_notification_lifecycle(cx, name);
        }
    }

    #[gpui::test]
    fn an_observer_without_an_object_hears_any_poster(cx: &mut gpui::TestAppContext) {
        // The accessibility display options arrive this way, from NSWorkspace's own center.
        let center = NSNotificationCenter::new();
        let name = objc2_foundation::NSString::from_str("OnicodeDisplayOptionsProbe");
        let calls = Rc::new(Cell::new(0));
        let _observer = NotificationObserver::new(
            center.clone(),
            &name,
            None,
            &cx.foreground_executor,
            Box::new({
                let calls = calls.clone();
                move || calls.set(calls.get() + 1)
            }),
        );
        unsafe { center.postNotificationName_object(&name, None) };
        cx.run_until_parked();
        assert_eq!(calls.get(), 1);
    }

    fn check_notification_lifecycle(
        cx: &mut gpui::TestAppContext,
        name: &'static NSNotificationName,
    ) {
        // Tests cannot share the default center because they run concurrently.
        let center = NSNotificationCenter::new();
        let calls = Rc::new(Cell::new(0));
        let observer = NotificationObserver::new(
            center.clone(),
            name,
            Some(&NSProcessInfo::processInfo()),
            &cx.foreground_executor,
            Box::new({
                let calls = calls.clone();
                move || calls.set(calls.get() + 1)
            }),
        );
        post_notification(&center, name);
        assert_eq!(calls.get(), 0);
        cx.run_until_parked();
        assert_eq!(calls.get(), 1);

        cx.background_executor
            .spawn({
                let center = center.clone();
                let name = name.to_string();
                async move {
                    post_notification(&center, &objc2_foundation::NSString::from_str(&name))
                }
            })
            .detach();
        assert_eq!(calls.get(), 1);
        cx.run_until_parked();
        assert_eq!(calls.get(), 2);

        post_notification(&center, name);
        drop(observer);
        cx.run_until_parked();
        assert_eq!(calls.get(), 2);
        post_notification(&center, name);
        cx.run_until_parked();
        assert_eq!(calls.get(), 2);
    }
}
