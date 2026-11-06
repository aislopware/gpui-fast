//! `gpui_tokio`, tracked from zed (`UPSTREAM`) because applications on this fork depend on
//! it as they do on zed's GPUI: onicode drives its network and agent clients on Tokio and
//! hands their results to views through it. zed's crate has no tests of its own
//! (`test = false`), and this repository leaves upstream manifests as zed has them, so its
//! behaviour is checked here, in gpui-fast's own crate: a future spawned on Tokio's pool
//! comes back as a GPUI task, and dropping that task aborts the Tokio task.

use std::{sync::mpsc, time::Duration};

use gpui_fast::TestAppContext;
use gpui_tokio::Tokio;

/// Sends on its channel when dropped, which a Tokio task does when it is aborted.
struct DropSignal(mpsc::Sender<()>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.send(()).ok();
    }
}

#[test]
fn a_tokio_future_comes_back_as_a_gpui_task() {
    let cx = TestAppContext::single();
    cx.update(gpui_tokio::init);
    // The value comes from a Tokio worker thread, which the test scheduler has to wait for.
    cx.executor().allow_parking();

    let task = Tokio::spawn(&cx, async { std::thread::current().id() });
    let tokio_thread = cx
        .foreground_executor()
        .block_test(task)
        .expect("the Tokio task ran");
    assert_ne!(tokio_thread, std::thread::current().id());
}

#[test]
fn dropping_the_gpui_task_aborts_the_tokio_task() {
    let cx = TestAppContext::single();
    cx.update(gpui_tokio::init);
    cx.executor().allow_parking();

    let (dropped, on_drop) = mpsc::channel();
    let (started, on_start) = mpsc::channel();
    let task = Tokio::spawn(&cx, async move {
        let _signal = DropSignal(dropped);
        started.send(()).ok();
        // Never finishes by itself: only an abort ends it.
        std::future::pending::<()>().await;
    });
    on_start
        .recv_timeout(Duration::from_secs(10))
        .expect("the Tokio task started");

    drop(task);
    // The GPUI task's future, holding the abort guard, is dropped as the scheduler runs
    // what the drop cancelled.
    cx.run_until_parked();
    on_drop
        .recv_timeout(Duration::from_secs(10))
        .expect("dropping the GPUI task aborted the Tokio task");
}
