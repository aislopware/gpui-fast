//! Interface zoom over the display scale (monocode's `Window::content_zoom`
//! and `set_content_zoom`; FORK.md, "Onicode").
//!
//! A window zoomed by `z` lays out against a viewport `z` times smaller and
//! paints at a scale factor `z` times larger, as a browser zooms a page: every
//! size the interface names, hard-coded pixels included, grows by `z`, and the
//! drawable keeps its size. GPUI's coordinates (its `Pixels`) are then the
//! platform's divided by `z`, so wherever the two meet they are mapped:
//!
//! - in: the input the platform sends ([`unzoomed`]), the content size and
//!   pointer the window reads on a resize ([`bounds_changed`]), the point an
//!   IME asks a character for, and the insets and visible viewport the
//!   platform reports;
//! - out: the rectangles an IME places its candidates by, a content size
//!   asked for, the point a window menu opens at, and where natives are
//!   placed and take the pointer (`fast::composition`).
//!
//! What the platform takes in device pixels needs nothing: the scene and the
//! accessibility tree's bounds are at the zoomed scale factor already, so a
//! device pixel stays one. Scroll layers' tiles are rastered at that scale and
//! dropped when it changes (`fast::layers::policy::drop_layers_on_resize`).
//! A cursor image keeps its own size in screen points, as the system's
//! cursors do. The window's own bounds on screen, and positions given to the
//! platform's window controls, stay the platform's.

use crate::{
    App, Bounds, FileDropEvent, Pixels, PlatformInput, Point, ScrollDelta, Size, Window,
    WindowInsets,
};

/// The smallest and largest zoom a window takes.
const ZOOM_RANGE: (f32, f32) = (0.25, 4.);

/// A window's interface zoom.
pub(crate) struct ContentZoom(f32);

impl Default for ContentZoom {
    fn default() -> Self {
        Self(1.)
    }
}

impl Window {
    /// The interface zoom set by [`Window::set_content_zoom`]; 1 unless set.
    pub fn content_zoom(&self) -> f32 {
        self.fast_content_zoom.0
    }

    /// Zooms everything the window draws, including hard-coded pixel sizes:
    /// layout runs against a viewport `zoom` times smaller, painting runs at a
    /// scale factor `zoom` times larger, and platform coordinates are mapped
    /// both ways. `zoom` is clamped to 0.25..=4.
    pub fn set_content_zoom(&mut self, zoom: f32, cx: &mut App) {
        let zoom = zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        if (zoom - self.fast_content_zoom.0).abs() < f32::EPSILON {
            return;
        }
        self.fast_content_zoom.0 = zoom;
        self.bounds_changed(cx);
    }
}

/// The window's scale, viewport and pointer read from the platform, zoomed
/// (the body of `Window::bounds_changed`, before it refreshes).
pub(crate) fn bounds_changed(window: &mut Window) {
    let zoom = window.fast_content_zoom.0;
    window.scale_factor = window.platform_window.scale_factor() * zoom;
    window.viewport_size = window.platform_window.content_size().map(|v| v / zoom);
    window.mouse_position = window.platform_window.mouse_position().map(|v| v / zoom);
}

/// The display's own scale: device pixels to the platform's point.
pub(crate) fn platform_scale(window: &Window) -> f32 {
    window.scale_factor / window.fast_content_zoom.0
}

/// A size in the window's coordinates, in the platform's.
pub(crate) fn size_out(window: &Window, size: Size<Pixels>) -> Size<Pixels> {
    let zoom = window.fast_content_zoom.0;
    size.map(|v| v * zoom)
}

/// A point in the window's coordinates, in the platform's.
pub(crate) fn point_out(window: &Window, point: Point<Pixels>) -> Point<Pixels> {
    let zoom = window.fast_content_zoom.0;
    point.map(|v| v * zoom)
}

/// A point in the platform's coordinates, in the window's.
pub(crate) fn point_in(point: Point<Pixels>, window: &Window) -> Point<Pixels> {
    let zoom = window.fast_content_zoom.0;
    point.map(|v| v / zoom)
}

/// Bounds in the window's coordinates, in the platform's.
pub(crate) fn bounds_out(window: &Window, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let zoom = window.fast_content_zoom.0;
    bounds.map(|v| v * zoom)
}

/// Bounds the platform reports, in the window's coordinates.
pub(crate) fn bounds_in(bounds: Bounds<Pixels>, window: &Window) -> Bounds<Pixels> {
    let zoom = window.fast_content_zoom.0;
    bounds.map(|v| v / zoom)
}

/// What an input handler answers for an IME's rectangle, in the platform's
/// coordinates (`PlatformInputHandler::bounds_for_range`, `selected_bounds`).
pub(crate) fn ime_bounds_out(
    bounds: Option<Bounds<Pixels>>,
    window: &Window,
) -> Option<Bounds<Pixels>> {
    bounds.map(|bounds| bounds_out(window, bounds))
}

/// The rectangles of a selection, in the platform's coordinates
/// (`PlatformInputHandler::selection_bounds_for_range`).
pub(crate) fn all_bounds_out(bounds: Vec<Bounds<Pixels>>, window: &Window) -> Vec<Bounds<Pixels>> {
    bounds
        .into_iter()
        .map(|bounds| bounds_out(window, bounds))
        .collect()
}

/// The insets the platform reports, in the window's coordinates.
pub(crate) fn insets_in(insets: WindowInsets, window: &Window) -> WindowInsets {
    let zoom = window.fast_content_zoom.0;
    WindowInsets {
        safe_area: insets.safe_area.map(|v| *v / zoom),
        ime: insets.ime.map(|v| *v / zoom),
        corner_radii: insets.corner_radii.map(|v| *v / zoom),
    }
}

/// Input from the platform, its positions and pixel deltas in the window's
/// coordinates (monocode's `PlatformInput::unzoomed`, with this fork's
/// gestures).
pub(crate) fn unzoomed(event: PlatformInput, window: &Window) -> PlatformInput {
    let zoom = window.fast_content_zoom.0;
    if zoom == 1. {
        return event;
    }
    let at = |p: Point<Pixels>| p.map(|v| v / zoom);
    match event {
        PlatformInput::MouseDown(mut e) => {
            e.position = at(e.position);
            PlatformInput::MouseDown(e)
        }
        PlatformInput::MouseUp(mut e) => {
            e.position = at(e.position);
            PlatformInput::MouseUp(e)
        }
        PlatformInput::MouseMove(mut e) => {
            e.position = at(e.position);
            PlatformInput::MouseMove(e)
        }
        PlatformInput::MousePressure(mut e) => {
            e.position = at(e.position);
            PlatformInput::MousePressure(e)
        }
        PlatformInput::MouseExited(mut e) => {
            e.position = at(e.position);
            PlatformInput::MouseExited(e)
        }
        PlatformInput::Scroll(mut e) => {
            e.position = at(e.position);
            if let ScrollDelta::Pixels(delta) = e.delta {
                e.delta = ScrollDelta::Pixels(at(delta));
            }
            PlatformInput::Scroll(e)
        }
        PlatformInput::Pinch(mut e) => {
            e.position = at(e.position);
            PlatformInput::Pinch(e)
        }
        PlatformInput::Gesture(mut gesture) => {
            match &mut gesture {
                crate::fast::gesture::PlatformGesture::Rotate(e) => e.position = at(e.position),
                crate::fast::gesture::PlatformGesture::SmartMagnify(e) => {
                    e.position = at(e.position)
                }
                crate::fast::gesture::PlatformGesture::Swipe(e) => e.position = at(e.position),
            }
            PlatformInput::Gesture(gesture)
        }
        PlatformInput::LongPress(mut e) => {
            e.start_position = at(e.start_position);
            e.position = at(e.position);
            PlatformInput::LongPress(e)
        }
        PlatformInput::TouchDrag(mut e) => {
            e.start_position = at(e.start_position);
            e.position = at(e.position);
            PlatformInput::TouchDrag(e)
        }
        PlatformInput::BackGesture(mut e) => {
            e.start_position = at(e.start_position);
            PlatformInput::BackGesture(e)
        }
        PlatformInput::Touch(mut e) => {
            e.position = at(e.position);
            e.predicted_position = e.predicted_position.map(at);
            PlatformInput::Touch(e)
        }
        PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }) => {
            PlatformInput::FileDrop(FileDropEvent::Entered {
                position: at(position),
                paths,
            })
        }
        PlatformInput::FileDrop(FileDropEvent::Pending { position }) => {
            PlatformInput::FileDrop(FileDropEvent::Pending {
                position: at(position),
            })
        }
        PlatformInput::FileDrop(FileDropEvent::Submit { position }) => {
            PlatformInput::FileDrop(FileDropEvent::Submit {
                position: at(position),
            })
        }
        event @ (PlatformInput::KeyDown(_)
        | PlatformInput::KeyUp(_)
        | PlatformInput::ModifiersChanged(_)
        | PlatformInput::FileDrop(FileDropEvent::Exited | FileDropEvent::Ended)) => event,
    }
}
