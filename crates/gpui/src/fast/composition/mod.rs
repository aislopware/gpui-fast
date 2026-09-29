//! Window composition: native platform views and layers (a WebView, a video
//! layer) drawn inside a GPUI window, ordered, clipped and faded with GPUI's
//! own content.
//!
//! A native's placement is part of the frame. The [`NativeView`] element
//! paints a [`NativePlacement`] into the scene at its place in painter's
//! order, next to the primitives around it, so a view drawn from the last frame
//! replays it like any primitive. Each placement also paints a hole: GPUI's
//! drawable is cleared where the native shows, and everything painted later
//! lands over the hole and so over the native. The platform keeps the natives
//! in containers under GPUI's layer and, once per presented frame, reconciles
//! them with the frame's placements ([`NativeFrame`]): a native no placement
//! named is hidden. Pointer input goes to a native only where it is the
//! topmost hitbox ([`NativeHitMap`]).
//!
//! `docs/composition.md` describes the model, the platform side and the
//! measurements behind it.

pub(crate) mod element;
pub(crate) mod frame;
pub(crate) mod hit_map;
pub(crate) mod scene;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_host;

use std::{any::Any, ffi::c_void, fmt, ptr::NonNull, rc::Rc};

use anyhow::Result;

use crate::{
    App, AppContext as _, Bounds, Corners, FocusHandle, Hitbox, Pixels, Scene, SharedString, Window,
};

pub use element::{NativeView, native_view};
pub use frame::{NativeFrame, NativeFramePlacement};
pub use hit_map::{NativeHitEntry, NativeHitMap};
pub use scene::{ComposedBatch, NativePlacement, SceneComposition};
#[cfg(any(test, feature = "test-support"))]
pub use test_host::TestNativeHost;

/// Identifies a [`NativeHost`] within its window. Hosts are numbered in the
/// order a window creates them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NativeId(pub u64);

/// How a [`NativeHost`] takes part in the window.
#[derive(Clone, Debug)]
pub struct NativeHostOptions {
    /// The native's content is opaque over its whole frame. GPUI cuts a hole
    /// where it shows, so GPUI content under a native is never seen through
    /// it; a translucent native is not supported yet and is drawn as if
    /// opaque.
    pub opaque: bool,
    /// The native takes pointer input where it is the topmost hitbox. When
    /// false, GPUI keeps all input over it.
    pub interactive: bool,
    /// What assistive technologies call the native.
    pub label: Option<SharedString>,
}

impl Default for NativeHostOptions {
    fn default() -> Self {
        Self {
            opaque: true,
            interactive: true,
            label: None,
        }
    }
}

/// A platform view or layer GPUI places, clips and orders inside its window.
///
/// Created once with [`Window::create_native_host`] and kept by whoever owns
/// the native content; placed every frame it should show by a
/// [`native_view`] element. The native is removed from the window when the
/// last clone of its host is dropped.
#[derive(Clone)]
pub struct NativeHost {
    id: NativeId,
    interactive: bool,
    platform: Rc<dyn PlatformNativeHost>,
}

impl NativeHost {
    /// The host's identity within its window.
    pub fn id(&self) -> NativeId {
        self.id
    }

    /// Makes `view` the host's content: an `NSView *` on macOS, a `UIView *`
    /// on iOS. The host's container adopts it as its only subview; the
    /// previous content, if any, is removed.
    ///
    /// # Safety
    ///
    /// `view` must point to a live view of this platform, and this must be
    /// called on the main thread.
    pub unsafe fn attach_view(&self, view: NonNull<c_void>) -> Result<()> {
        // SAFETY: forwarded under the caller's guarantee.
        unsafe { self.platform.attach_view(view) }
    }

    /// Makes `layer` (a `CALayer *`: a `CAMetalLayer`, an
    /// `AVSampleBufferDisplayLayer`, a plain layer) the host's content instead
    /// of a view.
    ///
    /// # Safety
    ///
    /// `layer` must point to a live `CALayer`, and this must be called on the
    /// main thread.
    pub unsafe fn attach_layer(&self, layer: NonNull<c_void>) -> Result<()> {
        // SAFETY: forwarded under the caller's guarantee.
        unsafe { self.platform.attach_layer(layer) }
    }

    /// The platform's side of this host.
    pub fn platform(&self) -> &Rc<dyn PlatformNativeHost> {
        &self.platform
    }
}

impl fmt::Debug for NativeHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NativeHost").field(&self.id).finish()
    }
}

/// What a platform window is asked to create a native host with.
pub struct NativeHostParams {
    /// The host's identity, which every [`NativeFrame`] names it by.
    pub id: NativeId,
    /// How the native takes part in the window.
    pub options: NativeHostOptions,
    /// For the platform to call, on the main thread and at any time, when the
    /// keyboard focus moved into the native (`true`: it or a view in it became
    /// first responder) or out of it (`false`) by the user's doing: GPUI then
    /// focuses the element tracking the native's focus, or takes its focus
    /// away. It defers the work, so it may be called from inside an event or a
    /// responder change.
    pub focused: Rc<dyn Fn(bool)>,
}

/// The platform's side of a [`NativeHost`]: a container in the window's view
/// tree that the platform places, clips and orders from each [`NativeFrame`].
pub trait PlatformNativeHost: Any {
    /// See [`NativeHost::attach_view`].
    ///
    /// # Safety
    ///
    /// As [`NativeHost::attach_view`].
    unsafe fn attach_view(&self, view: NonNull<c_void>) -> Result<()>;

    /// See [`NativeHost::attach_layer`].
    ///
    /// # Safety
    ///
    /// As [`NativeHost::attach_layer`].
    unsafe fn attach_layer(&self, layer: NonNull<c_void>) -> Result<()>;

    /// The container the native content lives in (an `NSView *` or a
    /// `UIView *`), where the platform has one.
    fn container(&self) -> Option<NonNull<c_void>> {
        None
    }

    /// This host as `Any`, for the platform and tests to reach their own type.
    fn as_any(&self) -> &dyn Any;
}

/// Everything a platform window needs to show one frame's natives with it.
pub struct NativePresent {
    /// Where each native shows, bottom to top.
    pub frame: NativeFrame,
    /// Where each native takes the pointer, for the platform's hit testing
    /// until the next present.
    pub hit_map: Rc<NativeHitMap>,
    /// The native GPUI's focus is on (its element tracks the focused handle),
    /// which should hold the platform's keyboard focus.
    pub keyboard: Option<NativeId>,
}

pub(crate) fn unsupported() -> Result<Rc<dyn PlatformNativeHost>> {
    anyhow::bail!("this platform cannot compose native views into a window")
}

/// A window's composition state.
#[derive(Default)]
pub(crate) struct WindowComposition {
    next_id: u64,
    /// A host was ever created, so frames are presented with their natives.
    active: bool,
    /// What the last present showed.
    presented: Option<NativeFrame>,
    presented_hit_map: Option<Rc<NativeHitMap>>,
}

impl Window {
    /// Creates a host for a native view or layer in this window. Fails on
    /// platforms that cannot compose native content into a window (Linux,
    /// Windows, the web), so a caller can fall back to something else.
    pub fn create_native_host(
        &mut self,
        options: NativeHostOptions,
        cx: &mut App,
    ) -> Result<NativeHost> {
        let id = NativeId(self.composition.next_id);
        let handle = self.handle;
        let executor = cx.foreground_executor().clone();
        let async_cx = cx.to_async();
        let focused: Rc<dyn Fn(bool)> = Rc::new(move |focused| {
            let mut cx = async_cx.clone();
            executor
                .spawn(async move {
                    cx.update_window(handle, |_, window, cx| {
                        window.native_focused(id, focused, cx)
                    })
                    .ok();
                })
                .detach();
        });
        let interactive = options.interactive;
        let platform = self.platform_window.create_native_host(NativeHostParams {
            id,
            options,
            focused,
        })?;
        self.composition.next_id += 1;
        self.composition.active = true;
        Ok(NativeHost {
            id,
            interactive,
            platform,
        })
    }

    /// Places `host`'s native at `bounds`, clipped by the current content
    /// mask, with its corners rounded by `corner_radii` and faded by the
    /// current element opacity. What is painted after this in the frame is
    /// drawn over the native. The native takes the pointer where `hitbox` is
    /// the topmost hitbox; without one, GPUI keeps the pointer over it.
    ///
    /// A native that no element places in a frame is hidden. [`native_view`]
    /// calls this; call it from a custom element's paint.
    pub fn paint_native(
        &mut self,
        host: &NativeHost,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        hitbox: Option<&Hitbox>,
    ) {
        self.paint_native_focused(host, bounds, corner_radii, hitbox, None);
    }

    pub(crate) fn paint_native_focused(
        &mut self,
        host: &NativeHost,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        hitbox: Option<&Hitbox>,
        focus: Option<&FocusHandle>,
    ) {
        self.invalidator.debug_assert_paint();
        let scale_factor = self.scale_factor();
        let placement = NativePlacement {
            order: 0,
            id: host.id,
            hitbox: hitbox.filter(|_| host.interactive).map(|hitbox| hitbox.id),
            focus: focus.map(|focus| focus.id),
            bounds: self.snap_bounds(bounds),
            content_mask: self.snapped_content_mask(),
            corner_radii: corner_radii.scale(scale_factor),
            opacity: self.element_opacity(),
        };
        self.next_frame.scene.insert_native(placement);
    }

    pub(crate) fn native_present(&self) -> NativePresent {
        let scene = &self.rendered_frame.scene;
        let frame = NativeFrame::new(scene, self.scale_factor(), self.viewport_size);
        let hit_map = Rc::new(NativeHitMap::new(
            &scene.composition,
            &self.rendered_frame.hitboxes,
        ));
        let keyboard = self.focus.and_then(|focus| {
            scene
                .composition
                .placements
                .iter()
                .rev()
                .find(|placement| placement.focus == Some(focus))
                .map(|placement| placement.id)
        });
        NativePresent {
            frame,
            hit_map,
            keyboard,
        }
    }

    /// The platform's keyboard focus moved into native `id` (`focused`) or out
    /// of it: focus the element tracking its focus, if one does, or blur it if
    /// it still has GPUI's focus.
    fn native_focused(&mut self, id: NativeId, focused: bool, cx: &mut App) {
        let focus = self
            .rendered_frame
            .scene
            .composition
            .placements
            .iter()
            .rev()
            .find(|placement| placement.id == id)
            .and_then(|placement| placement.focus);
        let Some(focus) = focus else {
            return;
        };
        if !focused {
            if self.focus == Some(focus) {
                self.blur(cx);
            }
            return;
        }
        if let Some(handle) = FocusHandle::for_id(focus, &cx.focus_handles) {
            self.focus(&handle, cx);
        }
    }

    /// Draws a frame and presents it, as the platform's frame callback does.
    #[cfg(test)]
    pub(crate) fn draw_and_present(&mut self, cx: &mut App) {
        self.draw(cx).clear(cx);
        present_scene(self);
    }

    /// Where the natives take the pointer, as of the last present.
    pub fn native_hit_map(&self) -> Option<Rc<NativeHitMap>> {
        self.composition.presented_hit_map.clone()
    }

    /// The natives the last present showed.
    pub fn presented_natives(&self) -> Option<&NativeFrame> {
        self.composition.presented.as_ref()
    }
}

/// Hands `window`'s rendered frame to the platform: with its natives once a
/// host exists, exactly as before otherwise.
pub(crate) fn present_scene(window: &mut Window) {
    if !window.composition.active {
        window.platform_window.draw(&window.rendered_frame.scene);
        return;
    }
    let present = window.native_present();
    window.composition.presented_hit_map = Some(present.hit_map.clone());
    window
        .platform_window
        .present_natives(&window.rendered_frame.scene, &present);
    window.composition.presented = Some(present.frame);
}

/// Whether the pointer is over a native that takes it, which then sets the
/// cursor itself: GPUI leaves the cursor alone there.
pub(crate) fn native_owns_cursor(window: &Window) -> bool {
    window.composition.active
        && window.mouse_hit_test.ids.first().is_some_and(|topmost| {
            window
                .rendered_frame
                .scene
                .composition
                .placements
                .iter()
                .any(|placement| placement.hitbox == Some(*topmost))
        })
}

impl Scene {
    /// The natives this scene places, with the holes they cut.
    pub fn natives(&self) -> &SceneComposition {
        &self.composition
    }
}
