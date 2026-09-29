//! Window composition on macOS: natives in containers under GPUI's view.
//!
//! Each [`NativeHost`](gpui::composition::NativeHost) is a `GPUINativeContainer`, a
//! layer-backed, flipped, clipping `NSView` added once to the window's content view
//! below the GPUI view and never removed or re-added while the host lives. Each
//! presented frame's [`NativeFrame`] sets the containers' frames, visibility and
//! stacking; GPUI's Metal layer, on top, has a hole cut wherever a native shows. A frame
//! that changes any of that is presented inside one explicit `CATransaction` with the
//! container changes, with the layer presenting with the transaction, so the hole and the
//! native move on the same refresh; other frames are presented as before.
//!
//! Input follows GPUI's hit test, published per presented frame as a
//! [`NativeHitMap`]: the GPUI view answers `hitTest:` with nil where a native is the
//! topmost hitbox, and a container only where its native is. GPUI still sees a click
//! that lands on a native (the window's `sendEvent:` hands it to GPUI first), and the
//! keyboard focus is kept in step both ways between a native and the focus handle its
//! element tracks.
//!
//! Everything here runs on the main thread, as AppKit and the window's views require.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    rc::{Rc, Weak},
    sync::Once,
};

use anyhow::Result;
use cocoa::{
    base::{id, nil},
    foundation::{NSPoint, NSRect, NSSize},
};
use gpui::{
    Bounds, Pixels, Scene,
    composition::{
        NativeFrame, NativeFramePlacement, NativeHitMap, NativeHostParams, NativeId, NativePresent,
        PlatformNativeHost,
    },
    point, px,
};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{BOOL, Class, NO, Object, Sel, YES},
    sel, sel_impl,
};

use gpui::WindowBackgroundAppearance;

use crate::window::MacWindow;

/// The `NSView` subclass holding a native.
static mut CONTAINER_CLASS: *const Class = std::ptr::null();

thread_local! {
    /// Each composing window's state, by its `NSWindow`.
    static WINDOWS: RefCell<HashMap<usize, Rc<WindowComposition>>> = RefCell::default();
    /// Each container's native and window, by the container `NSView`.
    static CONTAINERS: RefCell<HashMap<usize, (NativeId, Weak<WindowComposition>)>> =
        RefCell::default();
}

/// Whether frames that change natives are presented inside a Core Animation transaction
/// (`GPUI_COMPOSITION_TRANSACTIONS=0` turns it off, to see the seam it prevents).
fn transactional_frames() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("GPUI_COMPOSITION_TRANSACTIONS").map_or(true, |value| value != "0")
    })
}

/// One window's natives and what its last present showed of them.
pub(crate) struct WindowComposition {
    window: id,
    gpui_view: id,
    hosts: RefCell<HashMap<NativeId, Weak<MacNativeHost>>>,
    hit_map: RefCell<Rc<NativeHitMap>>,
    presented: RefCell<NativeFrame>,
    /// The native GPUI's focus asked the keyboard for in the last present.
    keyboard: Cell<Option<NativeId>>,
    /// A mouse button went down on a native, which gets the drag and the release.
    pressed_on_native: Cell<bool>,
    /// The first responder is being changed by this module, not by the user.
    moving_focus: Cell<bool>,
    transactional_presents: Cell<usize>,
    plain_presents: Cell<usize>,
}

impl WindowComposition {
    fn of_window(window: id) -> Option<Rc<Self>> {
        WINDOWS.with(|windows| windows.borrow().get(&(window as usize)).cloned())
    }

    fn host(&self, id: NativeId) -> Option<Rc<MacNativeHost>> {
        self.hosts.borrow().get(&id).and_then(Weak::upgrade)
    }

    /// Converts a point in the window's base coordinates to GPUI's.
    fn window_point_to_gpui(&self, location: NSPoint) -> gpui::Point<Pixels> {
        // SAFETY: the GPUI view is alive while its window composes; main thread.
        unsafe {
            let local: NSPoint = msg_send![self.gpui_view, convertPoint: location fromView: nil];
            self.view_point_to_gpui(local)
        }
    }

    /// Converts a point in the GPUI view's own coordinates to GPUI's.
    fn view_point_to_gpui(&self, local: NSPoint) -> gpui::Point<Pixels> {
        // SAFETY: as above.
        unsafe {
            let flipped: BOOL = msg_send![self.gpui_view, isFlipped];
            let bounds: NSRect = msg_send![self.gpui_view, bounds];
            let y = if flipped == YES {
                local.y
            } else {
                bounds.size.height - local.y
            };
            point(px(local.x as f32), px(y as f32))
        }
    }

    /// The native whose container holds `view`, if any.
    fn native_holding(&self, mut view: id) -> Option<NativeId> {
        // SAFETY: walks live views' superviews; main thread.
        unsafe {
            let view_class = class!(NSView);
            let is_view: BOOL = if view.is_null() {
                NO
            } else {
                msg_send![view, isKindOfClass: view_class]
            };
            if is_view == NO {
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

    /// The GPUI view's frame in the content view, which containers are placed in.
    fn frame_in_content_view(&self, bounds: Bounds<Pixels>) -> NSRect {
        // SAFETY: the GPUI view is alive; main thread.
        let gpui_frame: NSRect = unsafe { msg_send![self.gpui_view, frame] };
        let x = gpui_frame.origin.x + f64::from(bounds.origin.x);
        let top = f64::from(bounds.origin.y);
        let height = f64::from(bounds.size.height);
        let y = gpui_frame.origin.y + gpui_frame.size.height - top - height;
        NSRect::new(
            NSPoint::new(x, y),
            NSSize::new(f64::from(bounds.size.width), height),
        )
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
                    let () = msg_send![host.container, setHidden: YES];
                }
            }
        }
    }

    /// Gives a native's container and content their frames and stacking, and shows it.
    fn place(&self, host: &MacNativeHost, placement: &NativeFramePlacement, count: usize) {
        let content_frame = NSRect::new(
            NSPoint::new(
                f64::from(placement.bounds.origin.x - placement.visible.origin.x),
                f64::from(placement.bounds.origin.y - placement.visible.origin.y),
            ),
            NSSize::new(
                f64::from(placement.bounds.size.width),
                f64::from(placement.bounds.size.height),
            ),
        );
        // SAFETY: the container and its content are live views of this window; main
        // thread. Stacking is by `zPosition`, all below the GPUI view's layer at 0, so a
        // restack never removes a view from the window.
        unsafe {
            let () =
                msg_send![host.container, setFrame: self.frame_in_content_view(placement.visible)];
            let layer: id = msg_send![host.container, layer];
            let () = msg_send![layer, setZPosition: placement.rank as f64 - count as f64 - 1.];
            if let Some(content) = *host.content.borrow() {
                let () = msg_send![content, setFrame: content_frame];
            }
            let () = msg_send![host.container, setHidden: NO];
        }
    }

    /// Moves the platform's keyboard focus where GPUI's went, when GPUI moved it.
    fn follow_keyboard(&self, keyboard: Option<NativeId>) {
        let previous = self.keyboard.replace(keyboard);
        if previous == keyboard {
            return;
        }
        // SAFETY: the window and its views are live; main thread.
        unsafe {
            let responder: id = msg_send![self.window, firstResponder];
            let holder = self.native_holding(responder);
            let target = match keyboard {
                Some(native) if holder != Some(native) => self
                    .host(native)
                    .map(|host| host.content.borrow().unwrap_or(host.container)),
                None if previous.is_some() && holder == previous => Some(self.gpui_view),
                _ => None,
            };
            if let Some(target) = target {
                self.moving_focus.set(true);
                let _: BOOL = msg_send![self.window, makeFirstResponder: target];
                self.moving_focus.set(false);
            }
        }
    }
}

/// A native's container in a macOS window.
pub struct MacNativeHost {
    id: NativeId,
    container: id,
    content: RefCell<Option<id>>,
    focused: Rc<dyn Fn(bool)>,
    composition: Weak<WindowComposition>,
}

impl MacNativeHost {
    /// The container view (`NSView *`), for tests.
    pub fn container_view(&self) -> id {
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

    /// Where the window's natives take the pointer, as of its last present.
    pub fn hit_map(&self) -> Option<Rc<NativeHitMap>> {
        self.composition
            .upgrade()
            .map(|composition| composition.hit_map.borrow().clone())
    }

    fn set_content(&self, view: id) {
        // SAFETY: `view` is a live view the caller vouched for; main thread. The container
        // retains its subview; the host keeps its own reference until replaced or dropped.
        unsafe {
            if let Some(previous) = self.content.borrow_mut().take() {
                let () = msg_send![previous, removeFromSuperview];
                let () = msg_send![previous, release];
            }
            let _: id = msg_send![view, retain];
            let () = msg_send![self.container, addSubview: view];
        }
        *self.content.borrow_mut() = Some(view);
        // Placed already: the new content takes its frame now rather than at the next change.
        if let Some(composition) = self.composition.upgrade() {
            let presented = composition.presented.borrow();
            if let Some(placement) = presented.placement(self.id) {
                composition.place(self, placement, presented.placements.len());
            }
        }
    }
}

impl PlatformNativeHost for MacNativeHost {
    unsafe fn attach_view(&self, view: NonNull<c_void>) -> Result<()> {
        self.set_content(view.as_ptr().cast());
        Ok(())
    }

    unsafe fn attach_layer(&self, layer: NonNull<c_void>) -> Result<()> {
        // A layer-hosting view carries the layer, so AppKit places it like any view.
        // SAFETY: `layer` is a live CALayer the caller vouched for; main thread.
        unsafe {
            let view: id = msg_send![class!(NSView), alloc];
            let view: id = msg_send![view, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.))];
            let () = msg_send![view, setLayer: layer.as_ptr().cast::<Object>()];
            let () = msg_send![view, setWantsLayer: YES];
            self.set_content(view);
            let () = msg_send![view, release];
        }
        Ok(())
    }

    fn container(&self) -> Option<NonNull<c_void>> {
        NonNull::new(self.container.cast())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for MacNativeHost {
    fn drop(&mut self) {
        CONTAINERS.with(|containers| containers.borrow_mut().remove(&(self.container as usize)));
        if let Some(composition) = self.composition.upgrade() {
            composition.hosts.borrow_mut().remove(&self.id);
        }
        // SAFETY: the host owns one reference to its container and content; main thread.
        unsafe {
            if let Some(content) = self.content.borrow_mut().take() {
                let () = msg_send![content, release];
            }
            let () = msg_send![self.container, removeFromSuperview];
            let () = msg_send![self.container, release];
        }
    }
}

fn container_class() -> *const Class {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let mut decl = ClassDecl::new("GPUINativeContainer", class!(NSView))
            .expect("GPUINativeContainer is declared once");
        // SAFETY: the methods' signatures match the selectors' Objective-C types.
        unsafe {
            decl.add_method(sel!(isFlipped), yes as extern "C" fn(&Object, Sel) -> BOOL);
            decl.add_method(
                sel!(hitTest:),
                container_hit_test as extern "C" fn(&Object, Sel, NSPoint) -> id,
            );
            CONTAINER_CLASS = decl.register();
        }
    });
    // SAFETY: registered above, before any read.
    unsafe { CONTAINER_CLASS }
}

extern "C" fn yes(_: &Object, _: Sel) -> BOOL {
    YES
}

/// A container answers a hit test only where its native is the topmost hitbox.
extern "C" fn container_hit_test(this: &Object, _: Sel, point: NSPoint) -> id {
    let this_ptr = this as *const Object as usize;
    let Some((native, composition)) = CONTAINERS.with(|containers| {
        containers
            .borrow()
            .get(&this_ptr)
            .and_then(|(native, composition)| Some((*native, composition.upgrade()?)))
    }) else {
        return nil;
    };
    // SAFETY: `point` is in the superview's coordinates, as `hitTest:` passes it; main thread.
    let local: NSPoint = unsafe {
        let superview: id = msg_send![this, superview];
        msg_send![composition.gpui_view, convertPoint: point fromView: superview]
    };
    let position = composition.view_point_to_gpui(local);
    if composition.hit_map.borrow().native_at(position) != Some(native) {
        return nil;
    }
    // SAFETY: calls NSView's own hit test on this container.
    unsafe { msg_send![super(this, class!(NSView)), hitTest: point] }
}

/// The GPUI view declines a hit test where a native is the topmost hitbox, so AppKit
/// looks further, at the containers under it.
pub(crate) extern "C" fn gpui_view_hit_test(this: &Object, _: Sel, point: NSPoint) -> id {
    // SAFETY: `this` is the GPUI view; main thread.
    let window: id = unsafe { msg_send![this, window] };
    if let Some(composition) = WindowComposition::of_window(window) {
        // SAFETY: as above; `point` is in the superview's coordinates.
        let local: NSPoint = unsafe {
            let superview: id = msg_send![this, superview];
            msg_send![this, convertPoint: point fromView: superview]
        };
        let position = composition.view_point_to_gpui(local);
        if composition.hit_map.borrow().native_at(position).is_some() {
            return nil;
        }
    }
    // SAFETY: NSView's own hit test.
    unsafe { msg_send![super(this, class!(NSView)), hitTest: point] }
}

/// Whether a native holds the keyboard in the GPUI view's window, so GPUI's key
/// equivalent handling leaves keys to the native (they were offered to GPUI's keymap
/// already by the window's `sendEvent:`).
pub(crate) fn native_has_keyboard(gpui_view: &Object) -> bool {
    // SAFETY: `gpui_view` is the GPUI view; main thread.
    let window: id = unsafe { msg_send![gpui_view, window] };
    let Some(composition) = WindowComposition::of_window(window) else {
        return false;
    };
    // SAFETY: as above.
    let responder: id = unsafe { msg_send![window, firstResponder] };
    composition.native_holding(responder).is_some()
}

/// `NSWindow sendEvent:` for a composing window. Returns whether the event was consumed.
///
/// A mouse press that lands on a native is shown to GPUI first (so a menu open elsewhere
/// closes and the native's element hears it) and then goes on to the native, and so do
/// the drag and release that follow it. A key press while a native holds the keyboard,
/// and GPUI's focus is on that native's element, goes to GPUI's keymap first; a binding
/// that matches consumes it.
pub(crate) fn send_event(window: &Object, event: id) -> bool {
    let window_ptr = window as *const Object as id;
    let Some(composition) = WindowComposition::of_window(window_ptr) else {
        return false;
    };
    // SAFETY: `event` is the NSEvent being sent; main thread.
    let event_type: u64 = unsafe { msg_send![event, type] };
    const LEFT_DOWN: u64 = 1;
    const LEFT_UP: u64 = 2;
    const RIGHT_DOWN: u64 = 3;
    const RIGHT_UP: u64 = 4;
    const LEFT_DRAGGED: u64 = 6;
    const RIGHT_DRAGGED: u64 = 7;
    const KEY_DOWN: u64 = 10;
    const OTHER_DOWN: u64 = 25;
    const OTHER_UP: u64 = 26;
    const OTHER_DRAGGED: u64 = 27;
    match event_type {
        LEFT_DOWN | RIGHT_DOWN | OTHER_DOWN => {
            // SAFETY: as above.
            let location: NSPoint = unsafe { msg_send![event, locationInWindow] };
            let position = composition.window_point_to_gpui(location);
            if composition.hit_map.borrow().native_at(position).is_some() {
                composition.pressed_on_native.set(true);
                show_gpui(&composition, event);
            }
            false
        }
        LEFT_DRAGGED | RIGHT_DRAGGED | OTHER_DRAGGED if composition.pressed_on_native.get() => {
            show_gpui(&composition, event);
            false
        }
        LEFT_UP | RIGHT_UP | OTHER_UP if composition.pressed_on_native.get() => {
            composition.pressed_on_native.set(false);
            show_gpui(&composition, event);
            false
        }
        KEY_DOWN => {
            let Some(keyboard) = composition.keyboard.get() else {
                return false;
            };
            // SAFETY: as above.
            let responder: id = unsafe { msg_send![window_ptr, firstResponder] };
            if composition.native_holding(responder) != Some(keyboard) || composing_text(responder)
            {
                return false;
            }
            offer_key_to_gpui(composition.gpui_view, event)
        }
        _ => false,
    }
}

/// Whether `responder` is composing text with an input method, which keys must reach
/// untouched.
fn composing_text(responder: id) -> bool {
    // SAFETY: probes an NSResponder with `respondsToSelector:` first; main thread.
    unsafe {
        let responds: BOOL = msg_send![responder, respondsToSelector: sel!(hasMarkedText)];
        if responds == NO {
            return false;
        }
        let marked: BOOL = msg_send![responder, hasMarkedText];
        marked == YES
    }
}

/// Shows GPUI a mouse event AppKit delivers to a native, as if the GPUI view had it.
fn show_gpui(composition: &WindowComposition, event: id) {
    // SAFETY: the GPUI view is alive and `event` is the mouse event being sent; the view's
    // own handler converts it and dispatches it to GPUI, on the main thread.
    unsafe {
        crate::window::handle_view_event(&*composition.gpui_view, sel!(mouseDown:), event);
    }
}

/// Offers a key press to GPUI's keymap, as the GPUI view would dispatch it. Returns
/// whether a binding took it.
fn offer_key_to_gpui(gpui_view: id, event: id) -> bool {
    // SAFETY: the GPUI view holds its window state; main thread.
    let window_state = unsafe { crate::window::get_window_state(&*gpui_view) };
    let mut lock = window_state.lock();
    let window_height = lock.content_size().height;
    // SAFETY: `event` is the key event being sent.
    let Some(input) =
        (unsafe { crate::events::platform_input_from_native(event, Some(window_height)) })
    else {
        return false;
    };
    let Some(mut callback) = lock.event_callback.take() else {
        return false;
    };
    drop(lock);
    let result = callback(input);
    window_state.lock().event_callback = Some(callback);
    !result.propagate
}

/// `NSWindow makeFirstResponder:` for a composing window, after AppKit made the change:
/// tells GPUI when the user moved the keyboard into or out of a native.
pub(crate) fn first_responder_changed(window: &Object, previous: id, responder: id) {
    let Some(composition) = WindowComposition::of_window(window as *const Object as id) else {
        return;
    };
    if composition.moving_focus.get() {
        return;
    }
    let before = composition.native_holding(previous);
    let after = composition.native_holding(responder);
    if before == after {
        return;
    }
    if let Some(host) = before.and_then(|native| composition.host(native)) {
        (host.focused)(false);
    }
    if let Some(host) = after.and_then(|native| composition.host(native)) {
        (host.focused)(true);
    }
}

impl MacWindow {
    fn native_window_and_view(&self) -> (id, id) {
        let state = self.0.lock();
        (state.native_window, state.native_view.as_ptr())
    }

    fn draw_scene(&self, scene: &Scene) {
        self.0.lock().renderer.draw(scene);
    }

    /// Sets whether the GPUI layer presents with the transaction, returning what it did.
    fn set_presents_with_transaction(&self, with_transaction: bool) -> bool {
        let mut state = self.0.lock();
        let previous = state.renderer.presents_with_transaction();
        state
            .renderer
            .set_presents_with_transaction(with_transaction);
        previous
    }

    /// Whether GPUI's layer is opaque where it should be translucent over a hole, or the
    /// other way round, or shown where a native covers the window.
    fn gpui_layer_needs(&self, frame: &NativeFrame) -> bool {
        let state = self.0.lock();
        let Some(layer) = state.renderer.layer() else {
            return false;
        };
        let opaque =
            state.background_appearance == WindowBackgroundAppearance::Opaque && !frame.any_hole;
        // SAFETY: reads properties of the window's own Metal layer; main thread.
        let hidden: BOOL = unsafe { msg_send![layer, isHidden] };
        layer.is_opaque() != opaque || (hidden == YES) != frame.covers_window
    }

    /// Makes GPUI's layer composite over what is under it while a hole shows a native,
    /// and hides it while a native covers the window with nothing of GPUI's above.
    fn update_gpui_layer(&self, frame: &NativeFrame) {
        let state = self.0.lock();
        let Some(layer) = state.renderer.layer() else {
            return;
        };
        let opaque =
            state.background_appearance == WindowBackgroundAppearance::Opaque && !frame.any_hole;
        layer.set_opaque(opaque);
        // SAFETY: sets a property of the window's own Metal layer, inside the frame's
        // transaction; main thread.
        unsafe {
            let () = msg_send![layer, setHidden: if frame.covers_window { YES } else { NO }];
        }
    }

    /// Creates a native's container under the GPUI view.
    pub(crate) fn create_native_host_impl(
        &self,
        params: NativeHostParams,
    ) -> Result<Rc<dyn PlatformNativeHost>> {
        let (window, gpui_view) = self.native_window_and_view();
        let composition = WindowComposition::of_window(window).unwrap_or_else(|| {
            let composition = Rc::new(WindowComposition {
                window,
                gpui_view,
                hosts: RefCell::default(),
                hit_map: RefCell::default(),
                presented: RefCell::default(),
                keyboard: Cell::new(None),
                pressed_on_native: Cell::new(false),
                moving_focus: Cell::new(false),
                transactional_presents: Cell::new(0),
                plain_presents: Cell::new(0),
            });
            WINDOWS.with(|windows| {
                windows
                    .borrow_mut()
                    .insert(window as usize, composition.clone())
            });
            composition
        });
        // SAFETY: AppKit view creation and insertion on the main thread; the content view
        // retains the container, and the host keeps its own reference.
        let container = unsafe {
            let container: id = msg_send![container_class(), alloc];
            let container: id = msg_send![container, initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.))];
            let () = msg_send![container, setWantsLayer: YES];
            let () = msg_send![container, setClipsToBounds: YES];
            let () = msg_send![container, setHidden: YES];
            if let Some(label) = &params.options.label {
                let label = crate::ns_string(label);
                let () = msg_send![container, setAccessibilityLabel: label];
            }
            let content_view: id = msg_send![gpui_view, superview];
            let () = msg_send![content_view, addSubview: container positioned: -1isize relativeTo: gpui_view];
            container
        };
        CONTAINERS.with(|containers| {
            containers
                .borrow_mut()
                .insert(container as usize, (params.id, Rc::downgrade(&composition)))
        });
        let host = Rc::new(MacNativeHost {
            id: params.id,
            container,
            content: RefCell::new(None),
            focused: params.focused,
            composition: Rc::downgrade(&composition),
        });
        composition
            .hosts
            .borrow_mut()
            .insert(params.id, Rc::downgrade(&host));
        Ok(host)
    }

    /// Presents a frame with its natives.
    pub(crate) fn present_natives_impl(&self, scene: &Scene, present: &NativePresent) {
        let (window, _) = self.native_window_and_view();
        let Some(composition) = WindowComposition::of_window(window) else {
            self.draw_scene(scene);
            return;
        };
        *composition.hit_map.borrow_mut() = present.hit_map.clone();
        let frame = &present.frame;
        let changes_natives = !frame
            .changes_since(&composition.presented.borrow())
            .is_empty();
        let layer_changes = self.gpui_layer_needs(frame);
        if !changes_natives && !layer_changes {
            composition
                .plain_presents
                .set(composition.plain_presents.get() + 1);
            if !frame.covers_window {
                self.draw_scene(scene);
            }
        } else {
            composition
                .transactional_presents
                .set(composition.transactional_presents.get() + 1);
            let transaction = transactional_frames();
            // SAFETY: an explicit Core Animation transaction on the main thread, which
            // commits the drawable (presented with the transaction) together with the
            // containers' geometry; actions are disabled so nothing animates.
            unsafe {
                if transaction {
                    let () = msg_send![class!(CATransaction), begin];
                    let () = msg_send![class!(CATransaction), setDisableActions: YES];
                }
                let presented_with_transaction = self.set_presents_with_transaction(transaction);
                if !frame.covers_window {
                    self.draw_scene(scene);
                }
                self.set_presents_with_transaction(presented_with_transaction);
                composition.apply(frame);
                self.update_gpui_layer(frame);
                if transaction {
                    let () = msg_send![class!(CATransaction), commit];
                }
            }
        }
        *composition.presented.borrow_mut() = frame.clone();
        composition.follow_keyboard(present.keyboard);
    }
}

/// Registers the composition methods of the window classes, from their declaration.
///
/// # Safety
///
/// `decl` must be declaring an `NSWindow` subclass.
pub(crate) unsafe fn declare_window_methods(decl: &mut ClassDecl) {
    // SAFETY: the signatures match `sendEvent:` and `makeFirstResponder:`.
    unsafe {
        decl.add_method(
            sel!(sendEvent:),
            window_send_event as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(makeFirstResponder:),
            window_make_first_responder as extern "C" fn(&Object, Sel, id) -> BOOL,
        );
    }
}

/// The AppKit class GPUI's window class derives from. `object_getClass` may name a
/// subclass AppKit made at run time (key-value observing does), so the chain is walked up
/// to GPUI's own class rather than taking the immediate superclass.
fn window_superclass(this: &Object) -> &Class {
    let mut class = this.class();
    while !matches!(class.name(), "GPUIWindow" | "GPUIPanel") {
        class = class
            .superclass()
            .expect("a GPUI window derives from GPUI's window class");
    }
    class
        .superclass()
        .expect("GPUI's window class derives from NSWindow")
}

extern "C" fn window_send_event(this: &Object, _: Sel, event: id) {
    if send_event(this, event) {
        return;
    }
    // SAFETY: NSWindow's (or NSPanel's) own `sendEvent:`.
    unsafe {
        let () = msg_send![super(this, window_superclass(this)), sendEvent: event];
    }
}

extern "C" fn window_make_first_responder(this: &Object, _: Sel, responder: id) -> BOOL {
    // SAFETY: NSWindow's own `makeFirstResponder:` and `firstResponder`; main thread.
    unsafe {
        let previous: id = msg_send![this, firstResponder];
        let made: BOOL =
            msg_send![super(this, window_superclass(this)), makeFirstResponder: responder];
        if made == YES {
            first_responder_changed(this, previous, responder);
        }
        made
    }
}

/// Forgets a closing window's composition.
pub(crate) fn window_closed(window: id) {
    WINDOWS.with(|windows| windows.borrow_mut().remove(&(window as usize)));
}
