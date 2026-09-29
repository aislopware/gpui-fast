//! Window composition on iOS and iPadOS: natives in containers under GPUI's Metal view.
//!
//! The view controller's view is a `GPUIRootView` holding, bottom to top, one
//! `GPUINativeContainer` per [`NativeHost`](gpui::composition::NativeHost) and the
//! `GPUIMetalView` GPUI draws into, which cuts a hole wherever a native shows. A container is
//! added once and never removed or re-added while its host lives; each presented
//! [`NativeFrame`] sets the containers' frames, visibility and stacking. A frame that changes
//! any of that is presented inside one explicit `CATransaction` with the container changes,
//! with the Metal layer presenting with the transaction, so the hole and the native move on
//! the same refresh; other frames are presented as before.
//!
//! Input follows GPUI's hit test, published per presented frame as a [`NativeHitMap`]: the
//! Metal view answers `hitTest:withEvent:` with nil where a native is the topmost hitbox,
//! and a container only where its native is. A gesture recogniser on the root view that
//! never recognises and never cancels shows GPUI a touch that begins over a native as a
//! mouse press, so a menu open elsewhere closes and the native's element hears it. The
//! keyboard focus is kept in step both ways between a native and the focus handle its
//! element tracks.
//!
//! Everything here runs on the main thread, as UIKit and Core Animation require of views
//! and of the explicit transactions that move them.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ffi::c_void,
    ptr::{self, NonNull},
    rc::{Rc, Weak},
    sync::Once,
};

use anyhow::Result;
use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, PlatformInput, Point, Scene,
    composition::{
        NativeFrame, NativeFramePlacement, NativeHitMap, NativeHostParams, NativeId, NativePresent,
        PlatformNativeHost,
    },
    point, px,
};
use objc2::{
    class, msg_send,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel,
};

use super::IosWindow;
use super::cg_types::{ObjcCGPoint, ObjcCGRect};

const WINDOW_IVAR: &str = "gpui_window_ptr";
/// `UIGestureRecognizerStateFailed`, from UIGestureRecognizer.h.
const GESTURE_STATE_FAILED: isize = 5;

static ROOT_VIEW_CLASS: Once = Once::new();
static CONTAINER_CLASS: Once = Once::new();
static OBSERVER_CLASS: Once = Once::new();
static FIRST_RESPONDER_PROBE: Once = Once::new();

thread_local! {
    /// Each container's native and window, by the container `UIView`.
    static CONTAINERS: RefCell<HashMap<usize, (NativeId, Weak<WindowComposition>)>> =
        RefCell::default();
    /// Where `gpuiNoteFirstResponder:` found the first responder.
    static FIRST_RESPONDER: Cell<*mut AnyObject> = const { Cell::new(ptr::null_mut()) };
}

/// Whether frames that change natives are presented inside a Core Animation transaction
/// (`GPUI_COMPOSITION_TRANSACTIONS=0` turns it off, to see the seam it prevents).
fn transactional_frames() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GPUI_COMPOSITION_TRANSACTIONS").map_or(true, |value| value != "0")
    })
}

/// Creates the view controller's root view, the size of `frame`, holding `metal_view`.
///
/// # Safety
///
/// Main thread; `metal_view` is a live `GPUIMetalView` with no superview.
pub(crate) unsafe fn root_view(frame: ObjcCGRect, metal_view: *mut AnyObject) -> *mut AnyObject {
    // SAFETY: UIKit view creation on the main thread; the root retains the metal view.
    unsafe {
        let root: *mut AnyObject = msg_send![root_view_class(), alloc];
        let root: *mut AnyObject = msg_send![root, initWithFrame: frame];
        // UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight.
        let _: () = msg_send![root, setAutoresizingMask: 18_usize];
        let _: () = msg_send![root, addSubview: metal_view];
        let observer: *mut AnyObject = msg_send![observer_class(), alloc];
        let observer: *mut AnyObject =
            msg_send![observer, initWithTarget: ptr::null::<AnyObject>(), action: None::<Sel>];
        let _: () = msg_send![observer, setCancelsTouchesInView: false];
        let _: () = msg_send![observer, setDelaysTouchesBegan: false];
        let _: () = msg_send![observer, setDelaysTouchesEnded: false];
        let _: () = msg_send![root, addGestureRecognizer: observer];
        let _: () = msg_send![observer, release];
        root
    }
}

/// Points the root view at its window, or at nothing once the window is gone.
///
/// # Safety
///
/// Main thread; `root` is a live `GPUIRootView`, and `window` outlives the pointer or is null.
pub(crate) unsafe fn set_root_window(root: *mut AnyObject, window: *const IosWindow) {
    // SAFETY: the ivar was declared on the class `root` is an instance of.
    unsafe {
        #[allow(
            deprecated,
            reason = "objc2's ivar accessors are the only ones for a builder class"
        )]
        {
            *(*root).get_mut_ivar::<*mut c_void>(WINDOW_IVAR) = window.cast_mut().cast();
        }
    }
}

/// The window a root view serves, if it still does.
///
/// # Safety
///
/// Main thread; `root` is a live `GPUIRootView`.
unsafe fn window_of_root<'a>(root: *mut AnyObject) -> Option<&'a IosWindow> {
    // SAFETY: the ivar holds the window's stable boxed address from `register_with_ffi`
    // until `Drop` nulls it.
    unsafe {
        #[allow(
            deprecated,
            reason = "objc2's ivar accessors are the only ones for a builder class"
        )]
        let window: *mut c_void = *(*root).get_ivar(WINDOW_IVAR);
        (window as *const IosWindow).as_ref()
    }
}

fn root_view_class() -> &'static AnyClass {
    ROOT_VIEW_CLASS.call_once(|| {
        let Some(mut decl) = ClassBuilder::new(c"GPUIRootView", class!(UIView)) else {
            return;
        };
        decl.add_ivar::<*mut c_void>(c"gpui_window_ptr");
        decl.register();
    });
    class!(GPUIRootView)
}

fn container_class() -> &'static AnyClass {
    CONTAINER_CLASS.call_once(|| {
        let Some(mut decl) = ClassBuilder::new(c"GPUINativeContainer", class!(UIView)) else {
            return;
        };
        // SAFETY: the signature matches `hitTest:withEvent:`.
        unsafe {
            decl.add_method(
                sel!(hitTest:withEvent:),
                container_hit_test
                    as extern "C" fn(
                        *mut AnyObject,
                        Sel,
                        ObjcCGPoint,
                        *mut AnyObject,
                    ) -> *mut AnyObject,
            );
        }
        decl.register();
    });
    class!(GPUINativeContainer)
}

/// A gesture recogniser that watches touches over natives and never recognises, cancels,
/// delays or prevents anything.
fn observer_class() -> &'static AnyClass {
    OBSERVER_CLASS.call_once(|| {
        let Some(mut decl) =
            ClassBuilder::new(c"GPUINativeTouchObserver", class!(UIGestureRecognizer))
        else {
            return;
        };
        // SAFETY: the signatures match the selectors.
        unsafe {
            decl.add_method(
                sel!(touchesBegan:withEvent:),
                observer_touches_began
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(touchesEnded:withEvent:),
                observer_touches_ended
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(touchesCancelled:withEvent:),
                observer_touches_ended
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
            );
            decl.add_method(
                sel!(canPreventGestureRecognizer:),
                never as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool,
            );
            decl.add_method(
                sel!(canBePreventedByGestureRecognizer:),
                never as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool,
            );
        }
        decl.register();
    });
    class!(GPUINativeTouchObserver)
}

extern "C" fn never(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> Bool {
    Bool::NO
}

/// The composition of the window whose root view a recogniser is on.
fn composition_of_recognizer<'a>(recognizer: *mut AnyObject) -> Option<&'a IosWindow> {
    // SAFETY: the recogniser is on a live root view; main thread.
    unsafe {
        let root: *mut AnyObject = msg_send![recognizer, view];
        if root.is_null() {
            return None;
        }
        window_of_root(root)
    }
}

extern "C" fn observer_touches_began(
    this: *mut AnyObject,
    _: Sel,
    touches: *mut AnyObject,
    _event: *mut AnyObject,
) {
    let Some(window) = composition_of_recognizer(this) else {
        return;
    };
    let composition = &window.composition;
    for touch in touches_of(touches) {
        composition
            .active_touches
            .set(composition.active_touches.get() + 1);
        let position = composition.touch_position(touch);
        if composition.hit_map.borrow().native_at(position).is_some() {
            composition
                .touches_on_natives
                .borrow_mut()
                .insert(touch as usize);
            window.dispatch_input(PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }));
        }
    }
}

extern "C" fn observer_touches_ended(
    this: *mut AnyObject,
    _: Sel,
    touches: *mut AnyObject,
    _event: *mut AnyObject,
) {
    let Some(window) = composition_of_recognizer(this) else {
        return;
    };
    let composition = &window.composition;
    for touch in touches_of(touches) {
        composition
            .active_touches
            .set(composition.active_touches.get().saturating_sub(1));
        if composition
            .touches_on_natives
            .borrow_mut()
            .remove(&(touch as usize))
        {
            window.dispatch_input(PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: composition.touch_position(touch),
                modifiers: Modifiers::default(),
                click_count: 1,
            }));
        }
    }
    if composition.active_touches.get() == 0 {
        // SAFETY: a recogniser sets its own state from its touch callbacks; failing lets
        // UIKit reset it for the next sequence without it ever having recognised.
        unsafe {
            let _: () = msg_send![this, setState: GESTURE_STATE_FAILED];
        }
        // A tap may have moved the keyboard into or out of a native; UIKit has settled the
        // responder by the next turn of the main queue.
        let root: *mut AnyObject = unsafe { msg_send![this, view] };
        // SAFETY: retains the live root view for the queued check.
        let root = unsafe { objc2::rc::Retained::retain(root) };
        if let Some(root) = root {
            super::on_main(Box::new(move || {
                // SAFETY: the retained root is alive; main thread.
                if let Some(window) =
                    unsafe { window_of_root(objc2::rc::Retained::as_ptr(&root).cast_mut()) }
                {
                    window.composition.first_responder_changed();
                }
            }));
        }
    }
}

/// The `UITouch`es of an `NSSet`.
fn touches_of(touches: *mut AnyObject) -> Vec<*mut AnyObject> {
    // SAFETY: `touches` is the NSSet UIKit passed to a touch callback; main thread.
    unsafe {
        let all: *mut AnyObject = msg_send![touches, allObjects];
        let count: usize = msg_send![all, count];
        (0..count)
            .map(|index| msg_send![all, objectAtIndex: index])
            .collect()
    }
}

/// A container answers a hit test only where its native is the topmost hitbox.
extern "C" fn container_hit_test(
    this: *mut AnyObject,
    _: Sel,
    point_in_container: ObjcCGPoint,
    event: *mut AnyObject,
) -> *mut AnyObject {
    let Some((native, composition)) = CONTAINERS.with(|containers| {
        containers
            .borrow()
            .get(&(this as usize))
            .and_then(|(native, composition)| Some((*native, composition.upgrade()?)))
    }) else {
        return ptr::null_mut();
    };
    // SAFETY: converts between two live views of the same window; main thread.
    let in_metal_view: ObjcCGPoint = unsafe {
        msg_send![this, convertPoint: point_in_container, toView: composition.metal_view]
    };
    if composition
        .hit_map
        .borrow()
        .native_at(to_gpui(in_metal_view))
        != Some(native)
    {
        return ptr::null_mut();
    }
    // SAFETY: UIView's own hit test on this container.
    unsafe { msg_send![super(this, class!(UIView)), hitTest: point_in_container, withEvent: event] }
}

/// `GPUIMetalView hitTest:withEvent:`: nil where a native is the topmost hitbox, so UIKit
/// looks further, at the containers under the Metal view.
pub(crate) fn metal_view_hit_test(
    window: Option<&IosWindow>,
    this: *mut AnyObject,
    point_in_view: ObjcCGPoint,
    event: *mut AnyObject,
) -> *mut AnyObject {
    if let Some(window) = window
        && window
            .composition
            .hit_map
            .borrow()
            .native_at(to_gpui(point_in_view))
            .is_some()
    {
        return ptr::null_mut();
    }
    // SAFETY: UIView's own hit test on the Metal view; main thread.
    unsafe { msg_send![super(this, class!(UIView)), hitTest: point_in_view, withEvent: event] }
}

fn to_gpui(point_in_metal_view: ObjcCGPoint) -> Point<Pixels> {
    point(
        px(point_in_metal_view.x as f32),
        px(point_in_metal_view.y as f32),
    )
}

extern "C" fn note_first_responder(this: *mut AnyObject, _: Sel, _sender: *mut AnyObject) {
    FIRST_RESPONDER.with(|responder| responder.set(this));
}

/// The first responder of the key window, or null. UIKit has no getter; an action sent to
/// nil goes to the first responder, which records itself.
fn first_responder() -> *mut AnyObject {
    FIRST_RESPONDER_PROBE.call_once(|| {
        // SAFETY: adds a method to UIResponder whose implementation matches the `v@:@`
        // types given; it only records its receiver. Main thread, before any use.
        unsafe {
            let implementation: extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) =
                note_first_responder;
            objc2::ffi::class_addMethod(
                (class!(UIResponder) as *const AnyClass).cast_mut(),
                sel!(gpuiNoteFirstResponder:),
                std::mem::transmute::<
                    extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
                    objc2::runtime::Imp,
                >(implementation),
                c"v@:@".as_ptr(),
            );
        }
    });
    FIRST_RESPONDER.with(|responder| responder.set(ptr::null_mut()));
    // SAFETY: `sendAction:to:from:forEvent:` with a nil target dispatches along the responder
    // chain from the first responder; main thread.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(UIApplication), sharedApplication];
        let _: Bool = msg_send![application,
            sendAction: sel!(gpuiNoteFirstResponder:),
            to: ptr::null::<AnyObject>(),
            from: ptr::null::<AnyObject>(),
            forEvent: ptr::null::<AnyObject>()
        ];
    }
    FIRST_RESPONDER.with(Cell::get)
}

/// What a native shows: an attached view, or an attached layer as a sublayer of the
/// container's own (a `UIView`'s layer cannot be replaced).
enum Content {
    View(*mut AnyObject),
    Layer(*mut AnyObject),
}

impl Content {
    fn object(&self) -> *mut AnyObject {
        match self {
            Self::View(object) | Self::Layer(object) => *object,
        }
    }
}

/// One window's natives and what its last present showed of them.
pub(crate) struct WindowComposition {
    root: *mut AnyObject,
    metal_view: *mut AnyObject,
    hosts: RefCell<HashMap<NativeId, Weak<IosNativeHost>>>,
    hit_map: RefCell<Rc<NativeHitMap>>,
    presented: RefCell<NativeFrame>,
    /// The native GPUI's focus asked the keyboard for in the last present.
    keyboard: Cell<Option<NativeId>>,
    /// The native holding the first responder when it was last looked at.
    responder_native: Cell<Option<NativeId>>,
    active_touches: Cell<usize>,
    touches_on_natives: RefCell<HashSet<usize>>,
    transactional_presents: Cell<usize>,
    plain_presents: Cell<usize>,
}

impl WindowComposition {
    pub(crate) fn new(root: *mut AnyObject, metal_view: *mut AnyObject) -> Rc<Self> {
        Rc::new(Self {
            root,
            metal_view,
            hosts: RefCell::default(),
            hit_map: RefCell::default(),
            presented: RefCell::default(),
            keyboard: Cell::new(None),
            responder_native: Cell::new(None),
            active_touches: Cell::new(0),
            touches_on_natives: RefCell::default(),
            transactional_presents: Cell::new(0),
            plain_presents: Cell::new(0),
        })
    }

    fn host(&self, id: NativeId) -> Option<Rc<IosNativeHost>> {
        self.hosts.borrow().get(&id).and_then(Weak::upgrade)
    }

    fn touch_position(&self, touch: *mut AnyObject) -> Point<Pixels> {
        super::events::touch_location_in_view(touch, self.metal_view)
    }

    /// The native whose container holds `view`, if any.
    fn native_holding(&self, mut view: *mut AnyObject) -> Option<NativeId> {
        // SAFETY: walks live views' superviews; main thread.
        unsafe {
            if view.is_null() {
                return None;
            }
            let is_view: bool = msg_send![view, isKindOfClass: class!(UIView)];
            if !is_view {
                return None;
            }
            while !view.is_null() {
                if let Some(native) = CONTAINERS.with(|containers| {
                    containers
                        .borrow()
                        .get(&(view as usize))
                        .map(|(native, _)| *native)
                }) {
                    return Some(native);
                }
                view = msg_send![view, superview];
            }
            None
        }
    }

    /// Creates a native's container under the Metal view.
    pub(crate) fn create_host(
        self: &Rc<Self>,
        params: NativeHostParams,
    ) -> Result<Rc<dyn PlatformNativeHost>> {
        // SAFETY: UIKit view creation and insertion on the main thread; the root retains the
        // container, and the host keeps its own reference.
        let container = unsafe {
            let container: *mut AnyObject = msg_send![container_class(), alloc];
            let container: *mut AnyObject =
                msg_send![container, initWithFrame: ObjcCGRect::default()];
            let _: () = msg_send![container, setClipsToBounds: true];
            let _: () = msg_send![container, setHidden: true];
            if let Some(label) = &params.options.label {
                let _: () =
                    msg_send![container, setAccessibilityLabel: super::util::nsstring(label)];
            }
            let _: () =
                msg_send![self.root, insertSubview: container, belowSubview: self.metal_view];
            container
        };
        CONTAINERS.with(|containers| {
            containers
                .borrow_mut()
                .insert(container as usize, (params.id, Rc::downgrade(self)))
        });
        let host = Rc::new(IosNativeHost {
            id: params.id,
            container,
            content: RefCell::new(None),
            focused: params.focused,
            composition: Rc::downgrade(self),
        });
        self.hosts
            .borrow_mut()
            .insert(params.id, Rc::downgrade(&host));
        Ok(host)
    }

    /// Applies what changed since the last present to the containers. Called inside the
    /// frame's `CATransaction`.
    fn apply(&self, frame: &NativeFrame) {
        let presented = self.presented.borrow();
        let changes = frame.changes_since(&presented);
        for placement in changes.placed {
            if let Some(host) = self.host(placement.id) {
                self.place(&host, placement, frame.placements.len());
            }
        }
        for native in changes.hidden {
            if let Some(host) = self.host(native) {
                // SAFETY: the container is a live view of this window; main thread.
                unsafe {
                    let _: () = msg_send![host.container, setHidden: true];
                }
            }
        }
    }

    /// Gives a native's container and content their frames and stacking, and shows it.
    fn place(&self, host: &IosNativeHost, placement: &NativeFramePlacement, count: usize) {
        let visible = placement.visible;
        // SAFETY: the Metal view's frame in the root is where GPUI's coordinates start.
        let metal_frame: ObjcCGRect = unsafe { msg_send![self.metal_view, frame] };
        let container_frame = ObjcCGRect::new(
            metal_frame.x + f64::from(visible.origin.x),
            metal_frame.y + f64::from(visible.origin.y),
            f64::from(visible.size.width),
            f64::from(visible.size.height),
        );
        let content_frame = ObjcCGRect::new(
            f64::from(placement.bounds.origin.x - visible.origin.x),
            f64::from(placement.bounds.origin.y - visible.origin.y),
            f64::from(placement.bounds.size.width),
            f64::from(placement.bounds.size.height),
        );
        // SAFETY: the container and its content are live and this window's; main thread.
        // Stacking is by `zPosition`, all below the Metal view's layer at 0, so a restack
        // never removes a view from the window.
        unsafe {
            let _: () = msg_send![host.container, setFrame: container_frame];
            let layer: *mut AnyObject = msg_send![host.container, layer];
            let _: () = msg_send![layer, setZPosition: placement.rank as f64 - count as f64 - 1.];
            if let Some(content) = &*host.content.borrow() {
                let _: () = msg_send![content.object(), setFrame: content_frame];
            }
            let _: () = msg_send![host.container, setHidden: false];
        }
    }

    /// Tells GPUI when the user moved the keyboard into or out of a native.
    fn first_responder_changed(&self) {
        let now = self.native_holding(first_responder());
        let before = self.responder_native.replace(now);
        if before == now {
            return;
        }
        if let Some(host) = before.and_then(|native| self.host(native)) {
            (host.focused)(false);
        }
        if let Some(host) = now.and_then(|native| self.host(native)) {
            (host.focused)(true);
        }
    }

    /// Moves the platform's keyboard focus where GPUI's went, when GPUI moved it.
    fn follow_keyboard(&self, keyboard: Option<NativeId>) {
        let previous = self.keyboard.replace(keyboard);
        if previous == keyboard {
            return;
        }
        let holder = self.native_holding(first_responder());
        let target = match keyboard {
            Some(native) if holder != Some(native) => {
                self.host(native)
                    .and_then(|host| match &*host.content.borrow() {
                        Some(Content::View(view)) => Some(*view),
                        _ => None,
                    })
            }
            None if previous.is_some() && holder == previous => Some(self.metal_view),
            _ => None,
        };
        if let Some(target) = target {
            // SAFETY: a live view of this window takes the keyboard; main thread.
            let _: Bool = unsafe { msg_send![target, becomeFirstResponder] };
        }
        self.responder_native
            .set(self.native_holding(first_responder()));
    }
}

/// A native's container in an iOS window.
pub struct IosNativeHost {
    id: NativeId,
    container: *mut AnyObject,
    content: RefCell<Option<Content>>,
    focused: Rc<dyn Fn(bool)>,
    composition: Weak<WindowComposition>,
}

impl IosNativeHost {
    /// The container view (`UIView *`), for tests.
    pub fn container_view(&self) -> *mut AnyObject {
        self.container
    }

    /// Presents of this host's window that changed natives and were presented inside a
    /// transaction, and those that changed nothing.
    pub fn presents(&self) -> (usize, usize) {
        self.composition.upgrade().map_or((0, 0), |composition| {
            (
                composition.transactional_presents.get(),
                composition.plain_presents.get(),
            )
        })
    }

    fn set_content(&self, content: Content) {
        // SAFETY: `content` is a live view or layer the caller vouched for; main thread. The
        // container (or its layer) retains its child; the host keeps its own reference until
        // replaced or dropped. Actions are disabled so a standalone sublayer does not animate
        // into place.
        unsafe {
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            if let Some(previous) = self.content.borrow_mut().take() {
                match previous {
                    Content::View(view) => {
                        let _: () = msg_send![view, removeFromSuperview];
                    }
                    Content::Layer(layer) => {
                        let _: () = msg_send![layer, removeFromSuperlayer];
                    }
                }
                let _: () = msg_send![previous.object(), release];
            }
            let _: *mut AnyObject = msg_send![content.object(), retain];
            match content {
                Content::View(view) => {
                    let _: () = msg_send![self.container, addSubview: view];
                }
                Content::Layer(layer) => {
                    let container_layer: *mut AnyObject = msg_send![self.container, layer];
                    let _: () = msg_send![container_layer, addSublayer: layer];
                }
            }
            *self.content.borrow_mut() = Some(content);
            // Placed already: the new content takes its frame now rather than at the next
            // change.
            if let Some(composition) = self.composition.upgrade() {
                let presented = composition.presented.borrow();
                if let Some(placement) = presented.placement(self.id) {
                    composition.place(self, placement, presented.placements.len());
                }
            }
            let _: () = msg_send![class!(CATransaction), commit];
        }
    }
}

impl PlatformNativeHost for IosNativeHost {
    unsafe fn attach_view(&self, view: NonNull<c_void>) -> Result<()> {
        self.set_content(Content::View(view.as_ptr().cast()));
        Ok(())
    }

    unsafe fn attach_layer(&self, layer: NonNull<c_void>) -> Result<()> {
        self.set_content(Content::Layer(layer.as_ptr().cast()));
        Ok(())
    }

    fn container(&self) -> Option<NonNull<c_void>> {
        NonNull::new(self.container.cast())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for IosNativeHost {
    fn drop(&mut self) {
        CONTAINERS.with(|containers| containers.borrow_mut().remove(&(self.container as usize)));
        if let Some(composition) = self.composition.upgrade() {
            composition.hosts.borrow_mut().remove(&self.id);
        }
        // SAFETY: the host owns one reference to its container and content; main thread.
        unsafe {
            if let Some(content) = self.content.borrow_mut().take() {
                let _: () = msg_send![content.object(), release];
            }
            let _: () = msg_send![self.container, removeFromSuperview];
            let _: () = msg_send![self.container, release];
        }
    }
}

impl IosWindow {
    /// Presents a frame with its natives.
    pub(crate) fn present_natives_impl(&self, scene: &Scene, present: &NativePresent) {
        let composition = &self.composition;
        // The keyboard may have moved into or out of a native with no touch (a hardware
        // key, a web page's own focus change); GPUI learns of it before its next present.
        composition.first_responder_changed();
        *composition.hit_map.borrow_mut() = present.hit_map.clone();
        let frame = &present.frame;
        let changes_natives = !frame
            .changes_since(&composition.presented.borrow())
            .is_empty();
        let mut renderer = self.renderer.lock();
        let layer_changes = renderer.layer().is_some_and(|layer| {
            // SAFETY: reads a property of the window's own Metal layer; main thread.
            let hidden: bool = unsafe { msg_send![layer_object(layer), isHidden] };
            layer.is_opaque() == frame.any_hole || hidden != frame.covers_window
        });
        if !changes_natives && !layer_changes {
            composition
                .plain_presents
                .set(composition.plain_presents.get() + 1);
            if !frame.covers_window {
                renderer.draw(scene);
            }
        } else {
            composition
                .transactional_presents
                .set(composition.transactional_presents.get() + 1);
            let transaction = transactional_frames();
            // SAFETY: an explicit Core Animation transaction on the main thread, which commits
            // the drawable (presented with the transaction) together with the containers'
            // geometry; actions are disabled so nothing animates.
            unsafe {
                if transaction {
                    let _: () = msg_send![class!(CATransaction), begin];
                    let _: () = msg_send![class!(CATransaction), setDisableActions: true];
                }
                let presented_with_transaction = renderer.presents_with_transaction();
                renderer.set_presents_with_transaction(transaction);
                if !frame.covers_window {
                    renderer.draw(scene);
                }
                renderer.set_presents_with_transaction(presented_with_transaction);
                composition.apply(frame);
                if let Some(layer) = renderer.layer() {
                    layer.set_opaque(!frame.any_hole);
                    let _: () = msg_send![layer_object(layer), setHidden: frame.covers_window];
                }
                if transaction {
                    let _: () = msg_send![class!(CATransaction), commit];
                }
            }
        }
        drop(renderer);
        *composition.presented.borrow_mut() = frame.clone();
        composition.follow_keyboard(present.keyboard);
    }
}

fn layer_object(layer: &metal::MetalLayerRef) -> *mut AnyObject {
    use foreign_types::ForeignTypeRef as _;
    layer.as_ptr().cast()
}
