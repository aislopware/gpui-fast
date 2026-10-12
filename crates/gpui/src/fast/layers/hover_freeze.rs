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
//! the event was, hover is held still: of what was hovered when the wheel
//! began, what has stayed where it was (the panel around the list, the list
//! itself) stays hovered, and nothing else is, neither what the wheel moves
//! away nor what it brings under the pointer. The frame's hit test, and the
//! one a list foretells for its rows, hold only those. Nothing outside the
//! scrolled content is drawn again for its hover. When the wheel stops the
//! pointer is hit tested again and the window drawn, and what hovers
//! differently then is built again, nothing more. A pointer that moves ends
//! the freeze at once.

use std::time::{Duration, Instant};

use smallvec::SmallVec;

use crate::{App, Bounds, HitTest, Hitbox, HitboxBehavior, HitboxId, Pixels, Point, Window};

/// How long after the last wheel event hover stays held still.
pub(crate) const HOVER_FREEZE: Duration = Duration::from_millis(100);

/// The hitboxes hovered when a wheel began, each where it was then.
pub(crate) type Held = SmallVec<[(HitboxId, Bounds<Pixels>); 8]>;

/// When the window last got a wheel event, and where the pointer was.
pub(crate) struct HoverFreeze {
    last_wheel: Option<(Instant, Point<Pixels>)>,
    /// What was hovered when the wheel began: what of it stays where it was
    /// stays hovered while hover is held still.
    held: Held,
    /// Off in the crate's own tests, which scroll under a still pointer to
    /// test what hover does as rows move; see [`Window::set_hover_freeze`].
    enabled: bool,
}

impl Default for HoverFreeze {
    fn default() -> Self {
        Self {
            last_wheel: None,
            held: Held::new(),
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
            cx.update(|window, _| {
                if !frozen(window) {
                    thaw(window);
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
fn thaw(window: &mut Window) {
    window.fast_layers.hover_freeze.held.clear();
    window.mouse_hit_test = window.rendered_frame.hit_test(window.mouse_position);
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

/// Narrows the last frame's hit test as a frame begins to be drawn while
/// hover is held still, so that what is rendered counts as hovered only
/// what hover holds.
pub(crate) fn begin_draw(window: &mut Window) {
    if let Some(held) = holding(window) {
        let hit_test = held_hit_test(&window.rendered_frame.hitboxes, window.mouse_position, held);
        window.mouse_hit_test = hit_test;
    }
}

/// The hit test of the frame being drawn, taken between its prepaint and its
/// paint: of the hovered hitboxes, only what hover holds while it is held
/// still.
pub(crate) fn hit_test(window: &Window) -> HitTest {
    match holding(window) {
        Some(held) => held_hit_test(&window.next_frame.hitboxes, window.mouse_position, held),
        None => window.next_frame.hit_test(window.mouse_position),
    }
}

/// The hit test of `hitboxes` at `position`, as [`crate::Frame::hit_test`]
/// makes it, but for a hovered hitbox that `held` does not hold.
fn held_hit_test(
    hitboxes: &[Hitbox],
    position: Point<Pixels>,
    held: &[(HitboxId, Bounds<Pixels>)],
) -> HitTest {
    let mut hit_test = HitTest::default();
    let mut hover_hitbox_count = None;
    under_pointer(hitboxes, position, |hitbox, hovered| {
        if !hovered {
            hover_hitbox_count.get_or_insert(hit_test.ids.len());
            hit_test.ids.push(hitbox.id);
        } else if holds(Some(held), hitbox) {
            hit_test.ids.push(hitbox.id);
        }
    });
    hit_test.hover_hitbox_count = hover_hitbox_count.unwrap_or(hit_test.ids.len());
    hit_test
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
    }

    /// Ends holding hover still as the wheel stopping does, for the crate's
    /// tests, whose clock does not run.
    #[cfg(test)]
    pub(crate) fn end_hover_freeze(&mut self) {
        self.fast_layers.hover_freeze.last_wheel = None;
        thaw(self);
    }
}
