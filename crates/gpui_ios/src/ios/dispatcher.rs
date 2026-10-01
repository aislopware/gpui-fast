//! Main-queue helpers for UIKit work. Tasks themselves run on `gpui_apple`'s
//! `AppleDispatcher`, shared with macOS.

use dispatch2::{DispatchQueue, DispatchTime};
use std::{ffi::c_void, time::Duration};

/// Runs `f` on a later turn of the main queue.
pub(crate) fn on_main(f: Box<dyn FnOnce()>) {
    let context = Box::into_raw(Box::new(f)) as *mut c_void;
    // SAFETY: `boxed_trampoline` takes back the box leaked above, once, on the main queue.
    unsafe {
        DispatchQueue::main().exec_async_f(context, boxed_trampoline);
    }
}

/// Runs `f` on the main queue after `duration`.
pub(crate) fn after_on_main(duration: Duration, f: Box<dyn FnOnce()>) {
    let context = Box::into_raw(Box::new(f)) as *mut c_void;
    let when = DispatchTime::NOW.time(duration.as_nanos() as i64);
    // SAFETY: `boxed_trampoline` takes back the box leaked above, once, on the main queue.
    unsafe {
        DispatchQueue::exec_after_f(when, DispatchQueue::main(), context, boxed_trampoline);
    }
}

extern "C" fn boxed_trampoline(context: *mut c_void) {
    // SAFETY: `context` is the box `on_main` or `after_on_main` leaked, run exactly once.
    let f = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce()>) };
    f();
}
