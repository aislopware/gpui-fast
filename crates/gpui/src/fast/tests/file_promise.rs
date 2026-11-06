//! A drag carrying file promises reaches the platform whole, and its writers
//! report progress and cancellation. See [`crate::fast::file_promise`].

use std::{
    future::Future as _,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};

use futures::FutureExt as _;
use parking_lot::Mutex;

use crate::{
    AnyWindowHandle, AppContext as _, Context, Empty, ExternalDragPayload, FilePromise,
    InputEvent as _, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, PromiseProgress, Render, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, Window, div, point, px,
};

struct PromiseSource {
    promise: FilePromise,
}

impl Render for PromiseSource {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let promise = self.promise.clone();
        div()
            .id("promise-source")
            .size_full()
            .on_drag((), |_, _, _, cx| cx.new(|_| Empty))
            .external_drag_payload(move |_: &(), _, _| {
                Some(ExternalDragPayload::Promises(vec![promise.clone()]))
            })
    }
}

#[crate::test]
fn a_promise_drag_reaches_the_platform_and_writes_where_it_is_told(cx: &mut TestAppContext) {
    let written = Arc::new(Mutex::new(Vec::new()));
    let promise = FilePromise::new("report.pdf", "com.adobe.pdf", {
        let written = written.clone();
        move |path: PathBuf, progress: PromiseProgress| {
            let written = written.clone();
            async move {
                progress.set_total(10);
                progress.set_completed(10);
                written.lock().push(path);
                Ok(())
            }
            .boxed()
        }
    });
    let window: AnyWindowHandle = cx
        .add_window({
            let promise = promise.clone();
            move |_, _| PromiseSource { promise }
        })
        .into();
    cx.test_window(window).set_start_external_drag_result(true);

    let dragged = cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(
            MouseDownEvent {
                position: point(px(10.), px(10.)),
                button: MouseButton::Left,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        for position in [point(px(20.), px(20.)), point(px(-1.), px(20.))] {
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
        }
        // Leaving the window handed the drag to the platform.
        cx.active_drag.is_none()
    });
    assert_eq!(dragged.ok(), Some(true));

    let promises = cx.test_window(window).external_drag_promises();
    assert_eq!(promises, [promise]);
    assert_eq!(promises[0].file_name.as_ref(), "report.pdf");
    assert_eq!(promises[0].uti.as_ref(), "com.adobe.pdf");
    assert!(cx.test_window(window).external_drag_files().is_empty());

    let observed = Arc::new(Mutex::new(Vec::new()));
    let progress = PromiseProgress::observed({
        let observed = observed.clone();
        move |completed, total| observed.lock().push((completed, total))
    });
    let destination = PathBuf::from("/Volumes/Drop/report.pdf");
    let result =
        futures::executor::block_on((promises[0].write)(destination.clone(), progress.clone()));
    assert!(result.is_ok());
    assert_eq!(*written.lock(), [destination]);
    assert_eq!(*observed.lock(), [(0, 10), (10, 10)]);
    assert_eq!((progress.completed(), progress.total()), (10, 10));
}

#[test]
fn cancelling_wakes_a_writer_waiting_on_it() {
    let progress = PromiseProgress::default();
    let mut cancelled = progress.cancelled();
    let waker = futures::task::noop_waker();
    let mut task_cx = TaskContext::from_waker(&waker);
    assert_eq!(Pin::new(&mut cancelled).poll(&mut task_cx), Poll::Pending);
    assert!(!progress.is_cancelled());

    progress.cancel();
    assert!(progress.is_cancelled());
    assert_eq!(Pin::new(&mut cancelled).poll(&mut task_cx), Poll::Ready(()));
    // A writer that starts waiting after the cancel sees it at once.
    futures::executor::block_on(progress.cancelled());
}

#[test]
fn an_unchanged_count_does_not_report_again() {
    let observed = Arc::new(Mutex::new(0_usize));
    let progress = PromiseProgress::observed({
        let observed = observed.clone();
        move |_, _| *observed.lock() += 1
    });
    progress.set_total(5);
    progress.set_total(5);
    progress.set_completed(2);
    progress.set_completed(2);
    assert_eq!(*observed.lock(), 2);
}
