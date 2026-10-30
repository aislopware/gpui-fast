//! An input the platform hands in while the app is borrowed waits until the
//! borrow ends; it is never dropped.
//!
//! A platform delivers input from its own callbacks, and some of those run
//! inside GPUI's own work: UIKit cancels a touch, and sends the action of the
//! recognizer that followed it, the moment a view the touch was in leaves the
//! window or moves under it, which GPUI does while it draws or while a task
//! updates an entity. Upstream's input callback then finds the app borrowed,
//! logs `RefCell already borrowed` and drops the event: the end of a touch never
//! arrives, and the recognizer keeps a pan nothing lifts.
//!
//! Here such an event waits in the window's queue, every event after it waits
//! behind it so the window hears them in the order they came, and a task of
//! the queue's own, which the executor runs once the work that held the borrow
//! has returned, dispatches them. The platform is told that an event that
//! waits was handled, as the window will handle it.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

use gpui_util::ResultExt;

use crate::{AnyWindowHandle, AsyncApp, DispatchEventResult, PlatformInput};

/// The window's input callback, which the platform calls with each event (the
/// one `Window::new` hands `PlatformWindow::on_input`).
pub(crate) fn callback(
    window: AnyWindowHandle,
    cx: AsyncApp,
) -> Box<dyn FnMut(PlatformInput) -> DispatchEventResult> {
    let waiting = Rc::new(Waiting {
        window,
        cx,
        events: RefCell::default(),
        draining: Cell::new(false),
    });
    Box::new(move |event| deliver(&waiting, event))
}

/// One window's events waiting for the app.
struct Waiting {
    window: AnyWindowHandle,
    cx: AsyncApp,
    events: RefCell<VecDeque<PlatformInput>>,
    /// A task to dispatch `events` is spawned and has not finished.
    draining: Cell<bool>,
}

/// `event` dispatched now, or put behind the events waiting.
fn deliver(waiting: &Rc<Waiting>, event: PlatformInput) -> DispatchEventResult {
    let borrowed = borrowed_at(&waiting.cx);
    if borrowed.is_none() && waiting.events.borrow().is_empty() {
        return dispatch(waiting, event);
    }
    match borrowed {
        // Said where it is heard: a platform sending input from inside GPUI's work is
        // rare, and where the borrow began names the work.
        Some(origin) => log::info!(
            "a {} input waits for the app, borrowed from {origin}",
            event.kind_name()
        ),
        None => log::debug!(
            "a {} input waits behind the inputs before it",
            event.kind_name()
        ),
    }
    waiting.events.borrow_mut().push_back(event);
    drain_soon(waiting);
    DispatchEventResult::default()
}

/// `event` through the window, as upstream's callback dispatched it: the
/// frame it needs drawn first, then the event.
fn dispatch(waiting: &Waiting, event: PlatformInput) -> DispatchEventResult {
    let mut cx = waiting.cx.clone();
    crate::prepare_input(waiting.window, &event, &mut cx);
    waiting
        .window
        .update(&mut cx, |_, window, cx| window.dispatch_event(event, cx))
        .log_err()
        .unwrap_or_default()
}

/// Dispatches the waiting events from a task of their own, unless one is
/// spawned already.
fn drain_soon(waiting: &Rc<Waiting>) {
    if waiting.draining.replace(true) {
        return;
    }
    let held = Rc::clone(waiting);
    waiting
        .cx
        .foreground_executor()
        .spawn(async move { drain(&held) })
        .detach();
}

/// The waiting events, in order, each dispatched with the app free; an event
/// that comes meanwhile, even from inside one of their dispatches, goes behind
/// them and is dispatched in its turn.
fn drain(waiting: &Rc<Waiting>) {
    loop {
        if borrowed_at(&waiting.cx).is_some() {
            waiting.draining.set(false);
            drain_soon(waiting);
            return;
        }
        let next = waiting.events.borrow_mut().pop_front();
        let Some(event) = next else {
            break;
        };
        dispatch(waiting, event);
    }
    waiting.draining.set(false);
}

/// Where the borrow holding the app began, when the app is borrowed: the
/// place upstream's `RefCell already borrowed` panic names.
fn borrowed_at(cx: &AsyncApp) -> Option<String> {
    let app = cx.app.upgrade()?;
    if app.try_borrow_mut().is_ok() {
        return None;
    }
    Some(app.borrow_origin.get().map_or_else(
        || "an unknown place".to_owned(),
        |(origin, kind)| format!("{origin} ({kind})"),
    ))
}
