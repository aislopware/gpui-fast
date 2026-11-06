//! File promises on macOS.
//!
//! A drag out offers one `NSFilePromiseProvider` per [`FilePromise`]. Finder picks
//! the folder, AppKit asks the provider's delegate to write there on a background
//! queue, and the delegate publishes an `NSProgress` with the file's URL so Finder
//! draws its bar. A drop in that carries promises (Mail, Photos, Safari) receives
//! them into a staging folder with `NSFilePromiseReceiver`, then replays the drop
//! with the real paths, so drop targets see plain [`ExternalPaths`].

use std::{
    ffi::c_void,
    mem,
    path::PathBuf,
    ptr::{self, NonNull},
    rc::Rc,
    sync::Arc,
};

use block2::RcBlock;
use futures::channel::oneshot;
use gpui::{
    BackgroundExecutor, ExternalPaths, FileDropEvent, FilePromise, Pixels, Point, PromiseProgress,
};
use objc2::{
    AnyThread, ClassType, DefinedClass, Message, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSDraggingImageComponent, NSDraggingImageComponentIconKey, NSDraggingItem,
    NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSFilePromiseReceiver,
    NSPasteboardTypeFileURL, NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSCocoaErrorDomain, NSDictionary, NSError, NSFileWriteUnknownError, NSObject,
    NSObjectProtocol, NSOperationQueue, NSPoint, NSProgress, NSProgressFileOperationKindCopying,
    NSProgressKindFile, NSQualityOfService, NSRect, NSSize, NSString, NSURL, NSUserCancelledError,
};
use objc2_uniform_type_identifiers::{UTType, UTTypeData};
use parking_lot::Mutex;

use crate::window::{MacWindowState, send_file_drop_event};

/// The key the provider keeps its delegate under: AppKit holds the delegate weakly.
static DELEGATE_KEY: u8 = 0;

struct PromiseDelegateIvars {
    promise: FilePromise,
    executor: BackgroundExecutor,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements and `PromiseDelegate`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = PromiseDelegateIvars]
    struct PromiseDelegate;

    unsafe impl NSObjectProtocol for PromiseDelegate {}

    unsafe impl NSFilePromiseProviderDelegate for PromiseDelegate {
        // Called on the main thread while the drag is over a target, which
        // cancels the drag unless this returns at once.
        #[unsafe(method(filePromiseProvider:fileNameForType:))]
        fn file_name_for_type(
            &self,
            _provider: &NSFilePromiseProvider,
            _file_type: &NSString,
        ) -> Retained<NSString> {
            NSString::from_str(&self.ivars().promise.file_name)
        }

        // Called on the queue `operationQueueForFilePromiseProvider:` returns,
        // after the drop; the write may block it until done.
        #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
        fn write_promise_to_url(
            &self,
            _provider: &NSFilePromiseProvider,
            url: &NSURL,
            completion_handler: &block2::DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let ivars = self.ivars();
            let error = write_promise(&ivars.promise, &ivars.executor, url);
            let error = error
                .as_ref()
                .map_or(ptr::null_mut(), |error| Retained::as_ptr(error).cast_mut());
            completion_handler.call((error,));
        }

        #[unsafe(method(operationQueueForFilePromiseProvider:))]
        fn operation_queue(&self, _provider: &NSFilePromiseProvider) -> Retained<NSOperationQueue> {
            background_queue()
        }
    }
);

impl PromiseDelegate {
    fn new(promise: FilePromise, executor: BackgroundExecutor) -> Retained<Self> {
        let this = Self::alloc().set_ivars(PromiseDelegateIvars { promise, executor });
        // SAFETY: `NSObject`'s `init` is its designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

fn background_queue() -> Retained<NSOperationQueue> {
    let queue = NSOperationQueue::new();
    queue.setQualityOfService(NSQualityOfService::UserInitiated);
    queue
}

fn url_path(url: &NSURL) -> Option<PathBuf> {
    url.isFileURL()
        .then(|| url.path())
        .flatten()
        .map(|path| PathBuf::from(path.to_string()))
}

fn unit_count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Runs the promise's writer on the background executor, with Finder's
/// progress published for the destination, and blocks this queue thread until
/// it is done. Returns the error to hand AppKit, if any.
fn write_promise(
    promise: &FilePromise,
    executor: &BackgroundExecutor,
    url: &NSURL,
) -> Option<Retained<NSError>> {
    let Some(path) = url_path(url) else {
        log::warn!("file promise declined: destination is not a file URL");
        return Some(error(NSFileWriteUnknownError));
    };
    let progress = NSProgress::discreteProgressWithTotalUnitCount(-1);
    // SAFETY: AppKit's progress kinds are immutable `NSString` constants that
    // live as long as the process.
    unsafe {
        progress.setKind(Some(NSProgressKindFile));
        progress.setFileOperationKind(Some(NSProgressFileOperationKindCopying));
    }
    progress.setFileURL(Some(url));
    progress.setCancellable(true);
    let mirror = progress.clone();
    let writer_progress = PromiseProgress::observed(move |completed, total| {
        if total > 0 {
            mirror.setTotalUnitCount(unit_count(total));
        }
        mirror.setCompletedUnitCount(unit_count(completed));
    });
    let cancel = writer_progress.clone();
    let cancellation = RcBlock::new(move || cancel.cancel());
    // SAFETY: the handler only flips an atomic and wakes wakers, which is
    // safe from whichever thread Finder cancels on.
    unsafe { progress.setCancellationHandler(Some(&cancellation)) };
    progress.publish();

    let write = (promise.write)(path, writer_progress.clone());
    let result = futures::executor::block_on(executor.spawn(write));
    progress.unpublish();
    match result {
        Ok(()) => None,
        Err(_) if writer_progress.is_cancelled() => Some(error(NSUserCancelledError)),
        Err(err) => {
            log::warn!("file promise {:?} failed: {err:#}", promise.file_name);
            Some(error(NSFileWriteUnknownError))
        }
    }
}

fn error(code: isize) -> Retained<NSError> {
    // SAFETY: `NSCocoaErrorDomain` is an immutable constant, and a nil user
    // info dictionary is allowed.
    unsafe { NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, code, None) }
}

/// Starts a native drag offering `promises`, one provider each. Returns whether
/// AppKit began the session.
pub(crate) fn start_drag(state: &Mutex<MacWindowState>, promises: &[FilePromise]) -> bool {
    if promises.is_empty() {
        log::warn!("start_external_drag declined: no promises");
        return false;
    }
    let (native_view, native_window, mouse_down, executor) = {
        let state = state.lock();
        (
            state.native_view.as_ptr(),
            state.native_window,
            state.last_left_mouse_down_event.clone(),
            state.background_executor.clone(),
        )
    };
    let Some(mouse_down) = mouse_down else {
        log::warn!("start_external_drag declined: no retained left mouse down event");
        return false;
    };
    let event: *mut AnyObject = Retained::as_ptr(&mouse_down).cast_mut();

    // SAFETY: this runs on the main thread while AppKit dispatches the drag's
    // mouse move. The view and window are retained by the window state, and
    // every pointer handed to Objective-C stays valid for its call.
    unsafe {
        let location: NSPoint = msg_send![event, locationInWindow];
        // AppKit keeps this frame's distance from the event as the image's
        // offset from the cursor, so it stays anchored on `event`.
        let frame = NSRect::new(
            NSPoint::new(location.x - 16., location.y - 16.),
            NSSize::new(32., 32.),
        );
        let mut items = Vec::with_capacity(promises.len());
        for promise in promises {
            let uti = NSString::from_str(&promise.uti);
            let delegate = PromiseDelegate::new(promise.clone(), executor.clone());
            let provider = NSFilePromiseProvider::initWithFileType_delegate(
                NSFilePromiseProvider::alloc(),
                &uti,
                ProtocolObject::from_ref(&*delegate),
            );
            objc2::ffi::objc_setAssociatedObject(
                Retained::as_ptr(&provider).cast_mut().cast(),
                ptr::from_ref(&DELEGATE_KEY).cast::<c_void>(),
                Retained::as_ptr(&delegate).cast_mut().cast(),
                objc2::ffi::OBJC_ASSOCIATION_RETAIN,
            );
            let writer = ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*provider);
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);
            let content_type =
                UTType::typeWithIdentifier(&uti).unwrap_or_else(|| UTTypeData.retain());
            // Images resolve lazily, by type, as the file drag does.
            let images = RcBlock::new(move || {
                let component = NSDraggingImageComponent::draggingImageComponentWithKey(
                    NSDraggingImageComponentIconKey,
                );
                let icon = NSWorkspace::sharedWorkspace().iconForContentType(&content_type);
                component.setContents(Some(&icon));
                component.setFrame(NSRect::new(NSPoint::new(0., 0.), NSSize::new(32., 32.)));
                let components = NSArray::from_slice(&[&*component]);
                NonNull::new_unchecked(Retained::autorelease_return(components))
            });
            item.setDraggingFrame(frame);
            item.setImageComponentsProvider(Some(&images));
            items.push(item);
        }
        let items = NSArray::from_retained_slice(&items);
        let session: *mut AnyObject = msg_send![
            native_view,
            beginDraggingSessionWithItems: &*items,
            event: event,
            source: native_window
        ];
        let started = !session.is_null();
        if started {
            state.lock().synthetic_drag_counter += 1;
        }
        log::debug!(
            "start_external_drag (promises) completed: started={started}, item_count={}",
            promises.len()
        );
        started
    }
}

/// Registers the window for promised files and file URLs, beside the file
/// names it already accepts.
pub(crate) fn register_drag_types(native_window: *mut AnyObject) {
    let mut types = NSFilePromiseReceiver::readableDraggedTypes().to_vec();
    // SAFETY: the pasteboard type is an immutable constant.
    types.push(unsafe { NSPasteboardTypeFileURL }.retain());
    let types = NSArray::from_retained_slice(&types);
    // SAFETY: `native_window` is the live window being built, and
    // `registerForDraggedTypes:` adds to the types it already has.
    let () = unsafe { msg_send![native_window, registerForDraggedTypes: &*types] };
}

fn promise_receivers(
    dragging_info: *mut AnyObject,
) -> Option<Retained<NSArray<NSFilePromiseReceiver>>> {
    // SAFETY: `dragging_info` is the `NSDraggingInfo` AppKit passed the
    // destination method; classes are objects, so an array may hold one.
    unsafe {
        let pasteboard: *mut AnyObject = msg_send![dragging_info, draggingPasteboard];
        if pasteboard.is_null() {
            return None;
        }
        let class: &AnyClass = NSFilePromiseReceiver::class();
        let class = &*ptr::from_ref(class).cast::<AnyObject>();
        let classes = NSArray::from_slice(&[class]);
        let receivers: Option<Retained<NSArray<NSFilePromiseReceiver>>> = msg_send![
            pasteboard,
            readObjectsForClasses: &*classes,
            options: ptr::null::<NSDictionary>()
        ];
        receivers.filter(|receivers| receivers.count() > 0)
    }
}

/// What a drag of promised files offers while it hovers: no paths yet, so drop
/// targets can still highlight. `None` when the drag promises nothing.
pub(crate) fn promised_paths(dragging_info: *mut AnyObject) -> Option<ExternalPaths> {
    promise_receivers(dragging_info).map(|_| ExternalPaths::default())
}

struct Received {
    paths: Vec<PathBuf>,
    remaining: usize,
    done: Option<oneshot::Sender<Vec<PathBuf>>>,
}

/// Accepts a drop of promised files: receives them into a fresh staging folder
/// and replays the drop with their paths once all have arrived. `None` when
/// the drag promises nothing, so the caller handles it as a plain file drop.
pub(crate) fn receive(
    window_state: &Rc<Mutex<MacWindowState>>,
    dragging_info: *mut AnyObject,
    position: Point<Pixels>,
) -> Option<bool> {
    let receivers = promise_receivers(dragging_info)?;
    let expected: usize = receivers
        .iter()
        .map(|receiver| receiver.fileTypes().count())
        .sum();
    if expected == 0 {
        return Some(false);
    }
    let staging = std::env::temp_dir()
        .join("gpui-promises")
        .join(uuid::Uuid::new_v4().to_string());
    if let Err(err) = std::fs::create_dir_all(&staging) {
        log::warn!("file promise drop declined: staging folder: {err}");
        return Some(false);
    }
    let destination =
        NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&staging.to_string_lossy()), true);
    let (done, arrived) = oneshot::channel();
    let received = Arc::new(Mutex::new(Received {
        paths: Vec::with_capacity(expected),
        remaining: expected,
        done: Some(done),
    }));
    let queue = background_queue();
    let options: Retained<NSDictionary> = NSDictionary::new();
    for receiver in receivers.iter() {
        let received = received.clone();
        let reader = RcBlock::new(move |url: NonNull<NSURL>, error: *mut NSError| {
            let mut received = received.lock();
            if error.is_null() {
                // SAFETY: AppKit passes a valid URL for each file it read.
                if let Some(path) = url_path(unsafe { url.as_ref() }) {
                    received.paths.push(path);
                }
            } else {
                log::warn!("a promised file failed to arrive");
            }
            received.remaining = received.remaining.saturating_sub(1);
            if received.remaining == 0
                && let Some(done) = received.done.take()
            {
                let paths = mem::take(&mut received.paths);
                done.send(paths).ok();
            }
        });
        // SAFETY: the destination is a folder URL, the options dictionary is
        // empty, and the reader only touches state it owns.
        unsafe {
            receiver.receivePromisedFilesAtDestination_options_operationQueue_reader(
                &destination,
                &options,
                &queue,
                &reader,
            );
        }
    }

    let window = Rc::downgrade(window_state);
    let executor = window_state.lock().foreground_executor.clone();
    executor
        .spawn(async move {
            let Ok(paths) = arrived.await else {
                return;
            };
            let Some(window_state) = window.upgrade() else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let paths = ExternalPaths(paths.into_iter().collect());
            send_file_drop_event(
                window_state.clone(),
                FileDropEvent::Entered { position, paths },
            );
            send_file_drop_event(window_state.clone(), FileDropEvent::Submit { position });
            send_file_drop_event(window_state, FileDropEvent::Exited);
        })
        .detach();
    Some(true)
}
