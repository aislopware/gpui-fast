//! Hover held still while a wheel scrolls content under a still pointer.
//!
//! A wheel moves what is under the pointer without the pointer moving: every
//! frame of the scroll finds another row hovered, renders the row it leaves
//! and the row it enters again, and each may start an animation (a hover
//! card fading in, a toolbar showing) that keeps the list off its layer. A
//! frame of a long transcript scrolled by the wheel then costs about what it
//! costs without layers.
//!
//! So for [`HOVER_FREEZE`] after a wheel event, while the pointer stays where
//! the event was, hover styling is held still: of what was hovered when the
//! wheel began, what has stayed where it was (the panel around the list, the
//! list itself) stays hovered while a frame is drawn, and nothing else is,
//! neither what the wheel moves away nor what it brings under the pointer.
//! [`crate::HitboxId::is_hovered`] answers so while drawing, and the hit test a
//! list foretells for its rows holds only those. Nothing outside the scrolled
//! content is drawn again for its hover. Only drawing is held: the frame's hit
//! test stays the geometry under the pointer, so the cursor follows it, and
//! events dispatched between frames read it, so a click at the spot the wheel
//! scrolled under reaches its handler. When the wheel stops the window is
//! drawn again, and what hovers differently then is built again, nothing
//! more. A pointer that moves ends the freeze at once.

use scheduler::Instant;
use std::time::Duration;

use smallvec::SmallVec;

use crate::{App, Bounds, EntityId, Hitbox, HitboxBehavior, HitboxId, Pixels, Point, Window};

/// How long after the last wheel event hover stays held still.
pub(crate) const HOVER_FREEZE: Duration = Duration::from_millis(100);

/// The hitboxes hovered when a wheel began, each where it was then.
pub(crate) type Held = SmallVec<[(HitboxId, Bounds<Pixels>); 8]>;

/// When the window last got a wheel event, and where the pointer was.
#[derive(Clone)]
pub(crate) struct HoverFreeze {
    last_wheel: Option<(Instant, Point<Pixels>)>,
    /// What was hovered when the wheel began: what of it stays where it was
    /// stays hovered while hover is held still.
    held: Held,
    /// Of what is held, what the frame being drawn finds hovered where it was
    /// held: while hover is held still, what reads as hovered while drawing.
    pub(crate) drawn: SmallVec<[HitboxId; 8]>,
    /// Whether hover was held still as the frame being drawn began: asked
    /// once a frame, not on every hover read, since the clock is not free.
    drawing_held: bool,
    /// Off in the crate's own tests, which scroll under a still pointer to
    /// test what hover does as rows move; see [`Window::set_hover_freeze`].
    enabled: bool,
}

impl Default for HoverFreeze {
    fn default() -> Self {
        Self {
            last_wheel: None,
            held: Held::new(),
            drawn: SmallVec::new(),
            drawing_held: false,
            enabled: !cfg!(test),
        }
    }
}

/// Notes a wheel event at the pointer's position, taking in what is hovered
/// if it begins a scroll, and draws the window again once hover is no longer
/// held still for it.
pub(crate) fn note_wheel(window: &mut Window, cx: &mut App) {
    if !window.fast_layers.hover_freeze.enabled {
        return;
    }
    if !frozen(window) {
        let mut held = Held::new();
        under_pointer(
            &window.rendered_frame.hitboxes,
            window.mouse_position,
            |hitbox, hovered| {
                if hovered {
                    held.push((hitbox.id, hitbox.bounds));
                }
            },
        );
        window.fast_layers.hover_freeze.held = held;
    }
    window.fast_layers.hover_freeze.last_wheel = Some((Instant::now(), window.mouse_position));
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(HOVER_FREEZE + Duration::from_millis(10))
                .await;
            cx.update(|window, cx| {
                if !frozen(window) {
                    thaw(window, cx);
                }
            })
            .ok();
        })
        .detach();
}

/// Lets hover follow the pointer again: hit tests it against the frame shown
/// and marks the window to be drawn. The views whose hovers differ from what
/// they were drawn by are built again, as after a pointer move; the window is
/// not refreshed, which would build every view again.
///
/// The rows a list's layer kept through the wheel were painted by the hover
/// held, and a frame the layer composites paints none of them: the view
/// holding a list whose rows hover differently now is built again, which
/// renders those rows again, and only those.
fn thaw(window: &mut Window, cx: &mut App) {
    window.fast_layers.hover_freeze.held.clear();
    window.fast_layers.hover_freeze.drawn.clear();
    window.fast_layers.hover_freeze.drawing_held = false;
    let owners: SmallVec<[EntityId; 2]> = window
        .fast_layers
        .layers
        .values()
        .filter(|layer| {
            layer.rows.list && crate::fast::layers::lists::rows_hover_changed(window, layer)
        })
        .filter_map(|layer| layer.policy.owner())
        .collect();
    for owner in owners {
        cx.notify(owner);
    }
    if window.invalidator.not_drawing() {
        window.invalidator.set_dirty(true);
    }
}

/// Whether hover is held still: a wheel event came less than
/// [`HOVER_FREEZE`] ago, and the pointer has not moved since.
pub(crate) fn frozen(window: &Window) -> bool {
    window
        .fast_layers
        .hover_freeze
        .last_wheel
        .is_some_and(|(at, position)| {
            at.elapsed() < HOVER_FREEZE && position == window.mouse_position
        })
}

/// What hover holds while it is held still, `None` while it is not: the
/// hitboxes hovered when the wheel began, each where it was then. A hitbox
/// is hovered while hover is held still if it is under the pointer, it is
/// one of these, and it has not moved.
pub(crate) fn holding(window: &Window) -> Option<&[(HitboxId, Bounds<Pixels>)]> {
    frozen(window).then_some(window.fast_layers.hover_freeze.held.as_slice())
}

/// Whether `hitbox` may be hovered by what `held` holds (see [`holding`]).
pub(crate) fn holds(held: Option<&[(HitboxId, Bounds<Pixels>)]>, hitbox: &Hitbox) -> bool {
    held.is_none_or(|held| held.contains(&(hitbox.id, hitbox.bounds)))
}

/// Takes in, as a frame begins to be drawn, what of the last frame's
/// hitboxes hover holds while it is held still.
pub(crate) fn begin_draw(window: &mut Window) {
    let held = frozen(window);
    window.fast_layers.hover_freeze.drawing_held = held;
    let drawn = if held {
        held_hovered(window, &window.rendered_frame.hitboxes)
    } else {
        SmallVec::new()
    };
    window.fast_layers.hover_freeze.drawn = drawn;
}

/// Takes in, between the prepaint of the frame being drawn and its paint,
/// what of its hitboxes hover holds while it is held still.
pub(crate) fn begin_paint(window: &mut Window) {
    if window.fast_layers.hover_freeze.drawing_held {
        let drawn = if crate::fast::check_retained::scratch(window) {
            held_hovered_afresh(window)
        } else {
            held_hovered(window, &window.next_frame.hitboxes)
        };
        window.fast_layers.hover_freeze.drawn = drawn;
    }
}

/// The hovered hitboxes of `hitboxes` under the pointer that hover holds
/// where they were held.
fn held_hovered(window: &Window, hitboxes: &[Hitbox]) -> SmallVec<[HitboxId; 8]> {
    let held = window.fast_layers.hover_freeze.held.as_slice();
    let mut ids = SmallVec::new();
    under_pointer(hitboxes, window.mouse_position, |hitbox, hovered| {
        if hovered && holds(Some(held), hitbox) {
            ids.push(hitbox.id);
        }
    });
    ids
}

/// [`held_hovered`] for the retained check's frame from scratch, which drew
/// every hitbox afresh, under an id of its own: hover holds a hitbox of it
/// where the frame shown inserted, in the same place in its order and with
/// the same bounds, one that hover holds, which is the same element's.
fn held_hovered_afresh(window: &Window) -> SmallVec<[HitboxId; 8]> {
    let held = window.fast_layers.hover_freeze.held.as_slice();
    let shown = &window.rendered_frame.hitboxes;
    let afresh = &window.next_frame.hitboxes;
    let mut ids = SmallVec::new();
    under_pointer(afresh, window.mouse_position, |hitbox, hovered| {
        let at = afresh
            .iter()
            .position(|other| std::ptr::eq(other, hitbox))
            .unwrap_or(usize::MAX);
        if hovered
            && shown
                .get(at)
                .is_some_and(|was| was.bounds == hitbox.bounds && holds(Some(held), was))
        {
            ids.push(hitbox.id);
        }
    });
    ids
}

/// Whether hover reads as nothing for `hitbox`, which the pointer has not
/// captured, for [`crate::HitboxId::is_hovered`] and what a retained subtree
/// records of it: after keyboard input, as upstream has it, and while a frame
/// is drawn with hover held still, unless hover holds it where it was.
///
/// Event dispatch is not drawing. A mouse listener asks `is_hovered` before it
/// runs, so holding hover there too would drop a click made under a still
/// pointer right after the wheel.
pub(crate) fn hides_hover(window: &Window, hitbox: HitboxId) -> bool {
    window.last_input_was_keyboard()
        || (!window.invalidator.not_drawing() && held_away(window, hitbox))
}

/// Whether hover reads as nothing for `hitbox` in what a frame draws, as
/// [`hides_hover`] answers while drawing, whatever the phase: for what a
/// retained subtree records of a hover and the check, as a frame begins and
/// before its phases do, that the hovers it recorded still hold. Asked
/// outside drawing, the freeze would read as a hover changed on every frame
/// of a wheel, and every subtree under the pointer would be built again.
pub(crate) fn hides_drawn_hover(window: &Window, hitbox: HitboxId) -> bool {
    window.last_input_was_keyboard() || held_away(window, hitbox)
}

/// Whether hover was held still as the frame being drawn began, and does not
/// hold `hitbox` where it was.
fn held_away(window: &Window, hitbox: HitboxId) -> bool {
    let freeze = &window.fast_layers.hover_freeze;
    freeze.drawing_held && !freeze.drawn.contains(&hitbox)
}

/// Visits the hitboxes of `hitboxes` under `position`, topmost first, as
/// [`crate::Frame::hit_test`] finds them, with whether each counts as
/// hovered.
fn under_pointer(
    hitboxes: &[Hitbox],
    position: Point<Pixels>,
    mut visit: impl FnMut(&Hitbox, bool),
) {
    let mut hovering = true;
    for hitbox in hitboxes.iter().rev() {
        let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
        if !bounds.contains(&position) {
            continue;
        }
        visit(hitbox, hovering);
        if hitbox.behavior == HitboxBehavior::BlockMouseExceptScroll {
            hovering = false;
        }
        if hitbox.behavior == HitboxBehavior::BlockMouse {
            break;
        }
    }
}

impl Window {
    /// Turns holding hover still while a wheel scrolls on or off for this
    /// window, for the crate's tests.
    #[cfg(test)]
    pub(crate) fn set_hover_freeze(&mut self, enabled: bool) {
        self.fast_layers.hover_freeze.enabled = enabled;
        self.fast_layers.hover_freeze.last_wheel = None;
        self.fast_layers.hover_freeze.held.clear();
        self.fast_layers.hover_freeze.drawn.clear();
        self.fast_layers.hover_freeze.drawing_held = false;
    }

    /// Ends holding hover still as the wheel stopping does, for the crate's
    /// tests, whose clock does not run.
    #[cfg(test)]
    pub(crate) fn end_hover_freeze(&mut self, cx: &mut App) {
        self.fast_layers.hover_freeze.last_wheel = None;
        thaw(self, cx);
    }
}
