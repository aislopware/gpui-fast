//! HTML's `inert` for a subtree: it is drawn, but the pointer passes through
//! it as if it were not there.
//!
//! A popup playing its exit motion is the case this is for: Base UI's
//! positioner takes `pointer-events: none` once its popup is closing, so a
//! press during the fade neither picks an item of the menu on its way out nor
//! is lost on it, and reaches what lies beneath.
//!
//! Every hitbox inserted inside [`Window::with_inert`] (or under an [`Inert`]
//! element) gets an empty content mask: it is never hit, never blocks what
//! lies beneath, and nothing it guards hovers, presses, scrolls or sets the
//! cursor. Listeners that don't ask a hitbox (`on_mouse_down_out`, listeners
//! registered on the window) still run, as they do for an element under
//! another; a subtree leaving them on while inert must gate them itself.
//! Elements a subtree defers are drawn after it, outside its inert scope.

use crate::{
    AnyElement, App, Bounds, ContentMask, Element, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window,
};

/// The content mask a hitbox being inserted starts from: the current one, or
/// nothing at all inside an inert subtree.
pub(crate) fn mask(window: &Window) -> ContentMask<Pixels> {
    if window.fast_inert {
        ContentMask {
            bounds: Bounds::default(),
        }
    } else {
        window.content_mask()
    }
}

impl Window {
    /// Runs `f` with what it inserts inert, when `inert` is set: drawn, but
    /// never under the pointer. Inside an inert subtree everything stays
    /// inert.
    pub fn with_inert<R>(&mut self, inert: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.fast_inert;
        self.fast_inert = outer || inert;
        let result = f(self);
        self.fast_inert = outer;
        result
    }

    /// Whether what is being drawn now is inert.
    pub fn is_inert(&self) -> bool {
        self.fast_inert
    }
}

/// Draws `child` inert while `inert` is set. See [`Inert`].
pub fn inert(child: impl IntoElement, inert: bool) -> Inert {
    Inert {
        child: child.into_any_element(),
        inert,
    }
}

/// An element that draws its child as it is, inert while asked: the pointer
/// passes through the child to whatever lies beneath. It adds no layout node
/// of its own, so the child is laid out as if in its place.
pub struct Inert {
    child: AnyElement,
    inert: bool,
}

impl Element for Inert {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_inert(self.inert, |window| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_inert(self.inert, |window| child.paint(window, cx));
    }
}

impl IntoElement for Inert {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use crate::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, point, px,
    };

    struct Stack {
        inert: bool,
        clicks: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Stack {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            let over = self.clicks.clone();
            div()
                .size_full()
                .child(
                    div()
                        .id("beneath")
                        .absolute()
                        .size(px(100.))
                        .on_click(move |_, _, _| clicks.borrow_mut().push("beneath")),
                )
                .child(super::inert(
                    div()
                        .id("over")
                        .absolute()
                        .size(px(100.))
                        .occlude()
                        .on_click(move |_, _, _| over.borrow_mut().push("over")),
                    self.inert,
                ))
        }
    }

    /// The occluding element over the other takes the press; made inert, it
    /// lets the press through to the one beneath, and takes it back
    /// once it is no longer inert.
    #[crate::test]
    fn a_press_passes_through_an_inert_subtree_to_what_lies_beneath(cx: &mut TestAppContext) {
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let (view, cx) = cx.add_window_view(|_, _| Stack {
            inert: false,
            clicks: clicks.clone(),
        });
        let at = point(px(50.), px(50.));
        cx.simulate_click(at, Default::default());

        view.update(cx, |stack, cx| {
            stack.inert = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_click(at, Default::default());

        view.update(cx, |stack, cx| {
            stack.inert = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.simulate_click(at, Default::default());

        assert_eq!(*clicks.borrow(), ["over", "beneath", "over"]);
    }
}
