//! File promises: a drag that leaves the window with files that do not exist yet.
//!
//! The drop target picks a folder, then the platform calls each promise's writer
//! with the file's destination. The writer reports through [`PromiseProgress`] how
//! far it got, and learns there whether the receiver cancelled.

use std::{
    fmt,
    future::Future,
    mem,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
};

use futures::future::BoxFuture;
use parking_lot::Mutex;

use crate::SharedString;

/// Writes a promised file at the destination path it is given.
pub type FilePromiseWrite =
    Arc<dyn Fn(PathBuf, PromiseProgress) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

/// One file offered to a drop target, written only once the target accepts it.
#[derive(Clone)]
pub struct FilePromise {
    /// The file's name, as the drop target creates it.
    pub file_name: SharedString,
    /// The file's uniform type identifier, such as `public.data` or `public.folder`.
    pub uti: SharedString,
    /// Writes the file at the path the platform hands it, off the main thread.
    pub write: FilePromiseWrite,
}

impl FilePromise {
    /// A promise of `file_name`, of type `uti`, that `write` fulfils.
    pub fn new(
        file_name: impl Into<SharedString>,
        uti: impl Into<SharedString>,
        write: impl Fn(PathBuf, PromiseProgress) -> BoxFuture<'static, anyhow::Result<()>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            file_name: file_name.into(),
            uti: uti.into(),
            write: Arc::new(write),
        }
    }
}

impl fmt::Debug for FilePromise {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilePromise")
            .field("file_name", &self.file_name)
            .field("uti", &self.uti)
            .finish_non_exhaustive()
    }
}

/// Two promises are equal when they name the same file and share one writer.
impl PartialEq for FilePromise {
    fn eq(&self, other: &Self) -> bool {
        self.file_name == other.file_name
            && self.uti == other.uti
            && Arc::ptr_eq(&self.write, &other.write)
    }
}

impl Eq for FilePromise {}

type ProgressObserver = Box<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Default)]
struct ProgressState {
    total: AtomicU64,
    completed: AtomicU64,
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
    observer: Option<ProgressObserver>,
}

/// How far a promised file's write got, shared between the writer and the platform.
///
/// The writer sets the byte counts; the platform mirrors them where the receiver
/// draws its progress (Finder's bar on macOS), and cancels when the receiver does.
#[derive(Clone, Default)]
pub struct PromiseProgress(Arc<ProgressState>);

impl fmt::Debug for PromiseProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromiseProgress")
            .field("completed", &self.completed())
            .field("total", &self.total())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl PromiseProgress {
    /// A progress that calls `observer` with `(completed, total)` on every update.
    pub fn observed(observer: impl Fn(u64, u64) + Send + Sync + 'static) -> Self {
        Self(Arc::new(ProgressState {
            observer: Some(Box::new(observer)),
            ..ProgressState::default()
        }))
    }

    /// Sets the file's size in bytes, once it is known.
    pub fn set_total(&self, total: u64) {
        if self.0.total.swap(total, Ordering::AcqRel) != total {
            self.notify();
        }
    }

    /// Sets how many bytes are written so far.
    pub fn set_completed(&self, completed: u64) {
        if self.0.completed.swap(completed, Ordering::AcqRel) != completed {
            self.notify();
        }
    }

    /// The file's size in bytes, or zero while unknown.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.0.total.load(Ordering::Acquire)
    }

    /// The bytes written so far.
    #[must_use]
    pub fn completed(&self) -> u64 {
        self.0.completed.load(Ordering::Acquire)
    }

    /// Cancels the write: the receiver gave up on the file.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let wakers = mem::take(&mut *self.0.wakers.lock());
        for waker in wakers {
            waker.wake();
        }
    }

    /// Whether the receiver cancelled the write.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    /// Resolves once the receiver cancels the write.
    #[must_use]
    pub fn cancelled(&self) -> PromiseCancelled {
        PromiseCancelled(self.clone())
    }

    fn notify(&self) {
        if let Some(observer) = &self.0.observer {
            observer(self.completed(), self.total());
        }
    }
}

/// The future [`PromiseProgress::cancelled`] returns.
#[must_use = "futures do nothing unless polled"]
pub struct PromiseCancelled(PromiseProgress);

impl Future for PromiseCancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let state = &(self.0).0;
        if state.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        let mut wakers = state.wakers.lock();
        // `cancel` sets the flag before it takes the wakers, so a check under
        // the lock cannot miss it.
        if state.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if !wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
            wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// The promises a test window was asked to drag out.
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
#[derive(Default)]
pub(crate) struct TestDragPromises(Vec<FilePromise>);

/// Records a promise drag the test window started.
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub(crate) fn note_test_drag(recorded: &mut TestDragPromises, promises: &[FilePromise]) {
    recorded.0.extend_from_slice(promises);
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl crate::platform::TestWindow {
    /// The file promises this window's drags handed to the platform so far.
    pub(crate) fn external_drag_promises(&self) -> Vec<FilePromise> {
        self.0.lock().fast_drag_promises.0.clone()
    }
}
