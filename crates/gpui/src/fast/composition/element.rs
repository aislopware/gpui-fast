//! The element that places a native: a `div` whose content is the native.

use std::{cell::RefCell, panic, rc::Rc};

use refineable::Refineable as _;

use crate::{
    AnyElement, App, Bounds, Corners, Div, DivFrameState, Element, ElementId, FocusHandle,
    GlobalElementId, Hitbox, InspectorElementId, InteractiveElement, Interactivity, IntoElement,
    LayoutId, ParentElement, Pixels, Position, Style, StyleRefinement, Styled, Window, div,
};

use super::NativeHost;

/// Places `host`'s native in the element's bounds. The element is a `div`:
/// style it, give it an id, track a focus handle, listen to its events. Its
/// background is drawn under the native (a snapshot to show before the native
/// has drawn), its children and border over it, its corners round it and its
/// opacity fades it. It blocks the mouse from what is under it, and the native
/// takes the pointer wherever the element is the topmost hitbox.
///
/// A focus handle tracked with `track_focus` stands for the native's keyboard
/// focus: focusing it gives the native the keyboard, and the native taking
/// the keyboard focuses it. Keys go through the `NativeView` key context
/// first.
#[track_caller]
pub fn native_view(host: &NativeHost) -> NativeView {
    let slot = Rc::new(RefCell::new(None));
    let mut div = div()
        .key_context("NativeView")
        .child(PlacementElement { slot: slot.clone() });
    div.interactivity().occlude_mouse();
    NativeView {
        div,
        host: host.clone(),
        slot,
    }
}

/// The element [`native_view`] returns.
pub struct NativeView {
    div: Div,
    host: NativeHost,
    slot: Rc<RefCell<Option<Placement>>>,
}

struct Placement {
    host: NativeHost,
    bounds: Bounds<Pixels>,
    corner_radii: Corners<Pixels>,
    hitbox: Option<Hitbox>,
    focus: Option<FocusHandle>,
}

impl Styled for NativeView {
    fn style(&mut self) -> &mut StyleRefinement {
        self.div.style()
    }
}

impl InteractiveElement for NativeView {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.div.interactivity()
    }
}

impl ParentElement for NativeView {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.div.extend(elements);
    }
}

impl IntoElement for NativeView {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for NativeView {
    type RequestLayoutState = DivFrameState;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.div)
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        self.div.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.div.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.div
            .prepaint(id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let interactivity = self.div.interactivity();
        let mut style = Style::default();
        style.refine(&interactivity.base_style);
        let corner_radii = style
            .corner_radii
            .to_pixels(window.rem_size())
            .clamp_radii_for_quad_size(bounds.size);
        *self.slot.borrow_mut() = Some(Placement {
            host: self.host.clone(),
            bounds,
            corner_radii,
            hitbox: hitbox.clone(),
            focus: interactivity.tracked_focus_handle.clone(),
        });
        self.div
            .paint(id, inspector_id, bounds, request_layout, hitbox, window, cx);
        self.slot.borrow_mut().take();
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.div.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.div.write_a11y_info(node);
    }
}

/// The first child of a [`NativeView`]'s `div`: painted after the `div`'s
/// background and before its other children, it places the native.
struct PlacementElement {
    slot: Rc<RefCell<Option<Placement>>>,
}

impl IntoElement for PlacementElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PlacementElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        if let Some(placement) = self.slot.borrow().as_ref() {
            window.paint_native_focused(
                &placement.host,
                placement.bounds,
                placement.corner_radii,
                placement.hitbox.as_ref(),
                placement.focus.as_ref(),
            );
        }
    }
}
