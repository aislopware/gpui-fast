//! What a platform tells a window while the app is borrowed waits until the
//! borrow ends; it is never dropped.
//!
//! A platform tells a window of its input, its new size, its insets and the
//! like from its own callbacks, and some of those run inside GPUI's own work.
//! On iOS, GPUI presents a frame inside a Core Animation transaction, and UIKit
//! lays out the window's view controller as that transaction commits whenever
//! a native view GPUI placed has moved: `viewDidLayoutSubviews` reports the
//! window's insets, and its size when that changed, while the frame's draw
//! still holds the app. Upstream's callbacks then find the app borrowed, log
//! `RefCell already borrowed` and drop what they were told, so a size or an
//! inset that changed there never reaches the window, and an input sent there
//! is lost.
//!
//! Here each such word waits in a queue, every word after it waits behind it
//! so windows hear them in the order they came, and a task of the queue's own,
//! which the executor runs once the work that held the borrow has returned,
//! hands them on. The platform is told that an input that waits was handled,
//! as the window will handle it.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    panic::Location,
};

use gpui_util::ResultExt;

use crate::{AnyView, AnyWindowHandle, App, AsyncApp, DispatchEventResult, PlatformInput, Window};

/// The input callback a window hands its platform (`Window::new`'s
/// `PlatformWindow::on_input`).
#[track_caller]
pub(crate) fn input(
    window: AnyWindowHandle,
    cx: AsyncApp,
) -> Box<dyn FnMut(PlatformInput) -> DispatchEventResult> {
    let from = Location::caller();
    Box::new(move |event| match blocked(&cx) {
        Blocked::No => dispatch(window, &cx, event),
        blocked => {
            blocked.note(&format!("{} input", event.kind_name()), from);
            wait(window, &cx, Word::Input(event));
            DispatchEventResult::default()
        }
    })
}

/// `update` run on `window` now, as `AnyWindowHandle::update` would run it, or
/// once the app is free and the words before it have been heard: the body of
/// a platform callback of `Window::new`'s.
#[track_caller]
pub(crate) fn update(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    update: impl FnOnce(AnyView, &mut Window, &mut App) + 'static,
) {
    match blocked(cx) {
        Blocked::No => {
            window.update(cx, update).log_err();
        }
        blocked => {
            blocked.note("window update", Location::caller());
            wait(window, cx, Word::Update(Box::new(update)));
        }
    }
}

/// A platform's word to a window.
enum Word {
    Input(PlatformInput),
    Update(Box<dyn FnOnce(AnyView, &mut Window, &mut App)>),
}

/// A word waiting for its window's app.
struct Waiting {
    window: AnyWindowHandle,
    cx: AsyncApp,
    word: Word,
}

thread_local! {
    /// The words waiting, oldest first. The app's windows live on its main
    /// thread, and so do their platform callbacks.
    static WAITING: RefCell<VecDeque<Waiting>> = RefCell::default();
    /// A task to hand on `WAITING` is spawned and has not finished.
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

/// Why a word cannot be heard now.
enum Blocked {
    No,
    /// The app is borrowed, by the borrow that began here.
    Borrowed(String),
    /// Words that came before it wait.
    Behind,
}

impl Blocked {
    /// Says that a word waits, and why. A borrowed app is said where it is
    /// heard, since a platform telling a window anything from inside GPUI's
    /// work is rare and where the borrow began names the work.
    fn note(&self, what: &str, from: &Location<'_>) {
        match self {
            Self::No => {}
            Self::Borrowed(origin) => {
                log::info!("a {what} from {from} waits for the app, borrowed from {origin}");
            }
            Self::Behind => log::debug!("a {what} from {from} waits behind the words before it"),
        }
    }
}

fn blocked(cx: &AsyncApp) -> Blocked {
    if let Some(origin) = borrowed_at(cx) {
        return Blocked::Borrowed(origin);
    }
    if WAITING.with_borrow(VecDeque::is_empty) {
        Blocked::No
    } else {
        Blocked::Behind
    }
}

/// `event` through `window`, as upstream's callback dispatched it: the frame
/// it needs drawn first, then the event.
fn dispatch(window: AnyWindowHandle, cx: &AsyncApp, event: PlatformInput) -> DispatchEventResult {
    let mut cx = cx.clone();
    crate::prepare_input(window, &event, &mut cx);
    window
        .update(&mut cx, |_, window, cx| window.dispatch_event(event, cx))
        .log_err()
        .unwrap_or_default()
}

/// `word` behind the words waiting, and a task to hand them on unless one is
/// spawned already.
fn wait(window: AnyWindowHandle, cx: &AsyncApp, word: Word) {
    WAITING.with_borrow_mut(|waiting| {
        waiting.push_back(Waiting {
            window,
            cx: cx.clone(),
            word,
        });
    });
    drain_soon(cx);
}

fn drain_soon(cx: &AsyncApp) {
    if DRAINING.replace(true) {
        return;
    }
    cx.foreground_executor().spawn(async { drain() }).detach();
}

/// The waiting words, in order, each heard with its app free; a word that
/// comes meanwhile, even from inside one of their dispatches, goes behind them
/// and is heard in its turn.
fn drain() {
    loop {
        let next = WAITING.with_borrow_mut(|waiting| {
            let front = waiting.front()?;
            if borrowed_at(&front.cx).is_some() {
                return Some(Err(front.cx.clone()));
            }
            waiting.pop_front().map(Ok)
        });
        match next {
            None => break,
            Some(Err(cx)) => {
                DRAINING.set(false);
                drain_soon(&cx);
                return;
            }
            Some(Ok(Waiting { window, cx, word })) => match word {
                Word::Input(event) => {
                    dispatch(window, &cx, event);
                }
                Word::Update(update) => {
                    window.update(&mut cx.clone(), update).log_err();
                }
            },
        }
    }
    DRAINING.set(false);
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
