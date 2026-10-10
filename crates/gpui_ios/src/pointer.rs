//! A trackpad's or mouse's pointer on an iPad, as data: how it looks for each GPUI cursor
//! style, and which button a click is.
//!
//! The window's `UIPointerInteraction` asks its delegate for a `UIPointerStyle` whenever the
//! pointer enters the view or the style is invalidated; the platform's `set_cursor_style`
//! keeps the [`PointerLook`] it answers with. iPadOS draws no system cursor images, only
//! shapes: its own pointer (an arrow since iPadOS 26), a text beam, a hidden pointer, and any
//! outline path. So the resize, crosshair and similar cursors are outlines built here, and the
//! cursors iPadOS has no shape for (the hands, drag badges, a picture) keep its own pointer,
//! as iPad apps do.
//!
//! A click comes as a touch of type `UITouchTypeIndirectPointer`, with the event's
//! `buttonMask` naming the buttons held. Such a touch is a mouse button, not a finger: it goes
//! to GPUI as mouse events, as an `NSEvent` click does, so a click-drag selects rather than
//! scrolls (two fingers scroll, through the view's scroll recognizer) and a secondary click
//! (a two-finger click or tap on a trackpad, a mouse's right button) is the right button.
//! [`pointer_button`] tells the button; [`click_count`] counts double and triple clicks, which
//! UIKit leaves to the app.

use std::time::{Duration, Instant};

use gpui::{CursorStyle, MouseButton, Pixels, Point};

/// `UITouchTypeIndirectPointer`: a trackpad's or mouse's pointer, with
/// `UIApplicationSupportsIndirectInputEvents` set.
pub const TOUCH_TYPE_INDIRECT_POINTER: i64 = 3;

/// `UIEventButtonMaskPrimary`.
#[cfg(test)]
pub const BUTTON_MASK_PRIMARY: usize = 1 << 0;

/// `UIEventButtonMaskSecondary`.
pub const BUTTON_MASK_SECONDARY: usize = 1 << 1;

/// The length of the text beam, in points: about a line of 13 pt text.
pub const BEAM_LENGTH: f64 = 20.0;

/// How the pointer looks over the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerLook {
    /// iPadOS's own pointer.
    System,
    /// A text beam: upright for horizontal text, lying down for vertical text.
    Beam {
        /// The beam stands upright (`UIAxisVertical`).
        upright: bool,
    },
    /// No pointer: the view draws its own (a remote desktop's far-side cursor).
    Hidden,
    /// An outline ([`outline`]).
    Outline(Outline),
}

/// The outlines the pointer can take where iPadOS has no shape of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outline {
    /// A double arrow left and right: a column or a vertical edge resizes.
    LeftRight,
    /// A double arrow up and down: a row or a horizontal edge resizes.
    UpDown,
    /// A double arrow from top left to bottom right.
    UpLeftDownRight,
    /// A double arrow from top right to bottom left.
    UpRightDownLeft,
    /// A cross: a precise point.
    Cross,
}

/// The look of `style`.
pub const fn look(style: CursorStyle) -> PointerLook {
    match style {
        CursorStyle::IBeam => PointerLook::Beam { upright: true },
        CursorStyle::IBeamCursorForVerticalLayout => PointerLook::Beam { upright: false },
        CursorStyle::None => PointerLook::Hidden,
        CursorStyle::ResizeLeft
        | CursorStyle::ResizeRight
        | CursorStyle::ResizeLeftRight
        | CursorStyle::ResizeColumn => PointerLook::Outline(Outline::LeftRight),
        CursorStyle::ResizeUp
        | CursorStyle::ResizeDown
        | CursorStyle::ResizeUpDown
        | CursorStyle::ResizeRow => PointerLook::Outline(Outline::UpDown),
        CursorStyle::ResizeUpLeftDownRight => PointerLook::Outline(Outline::UpLeftDownRight),
        CursorStyle::ResizeUpRightDownLeft => PointerLook::Outline(Outline::UpRightDownLeft),
        CursorStyle::Crosshair => PointerLook::Outline(Outline::Cross),
        CursorStyle::Arrow
        | CursorStyle::PointingHand
        | CursorStyle::ClosedHand
        | CursorStyle::OpenHand
        | CursorStyle::OperationNotAllowed
        | CursorStyle::DragLink
        | CursorStyle::DragCopy
        | CursorStyle::ContextualMenu
        | CursorStyle::Image(_) => PointerLook::System,
    }
}

/// Half the length of a double arrow, in points.
const HALF: f64 = 11.0;
/// Half the width of an arrow's shaft.
const SHAFT: f64 = 1.5;
/// How far an arrow's head reaches back from its tip.
const HEAD: f64 = 5.0;
/// Half the width of an arrow's head.
const WING: f64 = 5.0;

/// The closed polygon of `outline`, centred on the hot spot, in points with y down: the
/// path the pointer's shape fills.
pub fn outline(outline: Outline) -> Vec<(f64, f64)> {
    match outline {
        Outline::LeftRight => double_arrow(),
        Outline::UpDown => turned(&double_arrow(), 0.0, 1.0),
        Outline::UpLeftDownRight => {
            let r = std::f64::consts::FRAC_1_SQRT_2;
            turned(&double_arrow(), r, r)
        }
        Outline::UpRightDownLeft => {
            let r = std::f64::consts::FRAC_1_SQRT_2;
            turned(&double_arrow(), r, -r)
        }
        Outline::Cross => {
            let (a, b) = (HALF, SHAFT);
            vec![
                (-b, -a),
                (b, -a),
                (b, -b),
                (a, -b),
                (a, b),
                (b, b),
                (b, a),
                (-b, a),
                (-b, b),
                (-a, b),
                (-a, -b),
                (-b, -b),
            ]
        }
    }
}

/// A double arrow along x, tips at ±[`HALF`].
fn double_arrow() -> Vec<(f64, f64)> {
    let neck = HALF - HEAD;
    vec![
        (-HALF, 0.0),
        (-neck, -WING),
        (-neck, -SHAFT),
        (neck, -SHAFT),
        (neck, -WING),
        (HALF, 0.0),
        (neck, WING),
        (neck, SHAFT),
        (-neck, SHAFT),
        (-neck, WING),
    ]
}

/// `points` turned so the x axis runs along `(cos, sin)`.
fn turned(points: &[(f64, f64)], cos: f64, sin: f64) -> Vec<(f64, f64)> {
    points
        .iter()
        .map(|&(x, y)| (x * cos - y * sin, x * sin + y * cos))
        .collect()
}

/// The button a `UITouchTypeIndirectPointer` touch presses: the right one when its event holds
/// the secondary button, else the left. `None` for a finger or a pencil, which go on as
/// touches.
pub const fn pointer_button(touch_type: i64, button_mask: usize) -> Option<MouseButton> {
    if touch_type != TOUCH_TYPE_INDIRECT_POINTER {
        None
    } else if button_mask & BUTTON_MASK_SECONDARY != 0 {
        Some(MouseButton::Right)
    } else {
        Some(MouseButton::Left)
    }
}

/// How long after a click the next one of the same button still counts on from it: AppKit's
/// default double-click interval.
pub const CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// How far from a click, in points, the next one still counts on from it.
pub const CLICK_SLOP: f64 = 4.0;

/// One press of a pointer button, kept to count the next one on from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    /// When it went down.
    pub at: Instant,
    /// Where.
    pub position: Point<Pixels>,
    /// Which button.
    pub button: MouseButton,
    /// Its count: 1 for a single click, 2 for a double.
    pub count: usize,
}

/// The count of a press of `button` at `position` at `now`, after `last`: one more than the
/// last's when it was the same button, within [`CLICK_INTERVAL`] and [`CLICK_SLOP`], else 1.
pub fn click_count(
    last: Option<Click>,
    now: Instant,
    position: Point<Pixels>,
    button: MouseButton,
) -> usize {
    match last {
        Some(last)
            if last.button == button
                && now.saturating_duration_since(last.at) <= CLICK_INTERVAL
                && (position - last.position).magnitude() <= CLICK_SLOP =>
        {
            last.count + 1
        }
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text takes the beam, a remote view's `None` hides the pointer, every resize cursor an
    /// outline along its axis, and the cursors iPadOS has no shape for keep its own pointer.
    #[test]
    fn each_cursor_has_its_look() {
        assert_eq!(
            look(CursorStyle::IBeam),
            PointerLook::Beam { upright: true }
        );
        assert_eq!(
            look(CursorStyle::IBeamCursorForVerticalLayout),
            PointerLook::Beam { upright: false }
        );
        assert_eq!(look(CursorStyle::None), PointerLook::Hidden);
        assert_eq!(look(CursorStyle::Arrow), PointerLook::System);
        assert_eq!(look(CursorStyle::PointingHand), PointerLook::System);
        for style in [
            CursorStyle::ResizeLeft,
            CursorStyle::ResizeRight,
            CursorStyle::ResizeLeftRight,
            CursorStyle::ResizeColumn,
        ] {
            assert_eq!(
                look(style),
                PointerLook::Outline(Outline::LeftRight),
                "{style:?}"
            );
        }
        for style in [
            CursorStyle::ResizeUp,
            CursorStyle::ResizeDown,
            CursorStyle::ResizeUpDown,
            CursorStyle::ResizeRow,
        ] {
            assert_eq!(
                look(style),
                PointerLook::Outline(Outline::UpDown),
                "{style:?}"
            );
        }
        assert_eq!(
            look(CursorStyle::Crosshair),
            PointerLook::Outline(Outline::Cross)
        );
    }

    /// Each outline is a closed polygon centred on the hot spot, as wide one way as the other,
    /// and the up-down arrow is the left-right one stood up.
    #[test]
    fn outlines_are_centred_on_the_hot_spot() {
        for shape in [
            Outline::LeftRight,
            Outline::UpDown,
            Outline::UpLeftDownRight,
            Outline::UpRightDownLeft,
            Outline::Cross,
        ] {
            let points = outline(shape);
            assert!(points.len() >= 10, "{shape:?}");
            let n = points.len() as f64;
            let (cx, cy) = points
                .iter()
                .fold((0.0, 0.0), |(x, y), p| (x + p.0 / n, y + p.1 / n));
            assert!(
                cx.abs() < 1e-9 && cy.abs() < 1e-9,
                "{shape:?}: ({cx}, {cy})"
            );
            let reach = points.iter().map(|p| p.0.hypot(p.1)).fold(0.0, f64::max);
            assert!(
                (reach - HALF.hypot(if shape == Outline::Cross { SHAFT } else { 0.0 })).abs()
                    < 1e-9
            );
        }
        let tall = outline(Outline::UpDown);
        assert!(
            tall.iter().all(|p| p.0.abs() <= WING + 1e-9),
            "narrow across"
        );
        assert!(
            tall.iter().any(|p| (p.1 - HALF).abs() < 1e-9),
            "tips up and down"
        );
    }

    /// A pointer's secondary button is a right click and its primary a left one; a finger and
    /// a pencil go on as touches.
    #[test]
    fn a_secondary_pointer_click_is_the_right_button() {
        let pointer = TOUCH_TYPE_INDIRECT_POINTER;
        assert_eq!(
            pointer_button(pointer, BUTTON_MASK_SECONDARY),
            Some(MouseButton::Right)
        );
        assert_eq!(
            pointer_button(pointer, BUTTON_MASK_PRIMARY | BUTTON_MASK_SECONDARY),
            Some(MouseButton::Right)
        );
        assert_eq!(
            pointer_button(pointer, BUTTON_MASK_PRIMARY),
            Some(MouseButton::Left)
        );
        assert_eq!(pointer_button(0, BUTTON_MASK_SECONDARY), None, "a finger");
        assert_eq!(pointer_button(2, BUTTON_MASK_SECONDARY), None, "a pencil");
    }

    /// A second press of the same button soon after and near the first is a double click,
    /// and a third a triple; another button, a press too late or too far starts again at one.
    #[test]
    fn presses_close_together_count_up() {
        use gpui::{point, px};
        let start = Instant::now();
        let at = point(px(100.0), px(50.0));
        let first = Click {
            at: start,
            position: at,
            button: MouseButton::Left,
            count: 1,
        };
        let soon = start + Duration::from_millis(200);
        assert_eq!(click_count(None, start, at, MouseButton::Left), 1);
        assert_eq!(click_count(Some(first), soon, at, MouseButton::Left), 2);
        let second = Click {
            count: 2,
            at: soon,
            ..first
        };
        let near = point(px(102.0), px(51.0));
        let later = soon + Duration::from_millis(200);
        assert_eq!(click_count(Some(second), later, near, MouseButton::Left), 3);
        assert_eq!(
            click_count(Some(first), soon, at, MouseButton::Right),
            1,
            "another button"
        );
        let late = start + Duration::from_millis(600);
        assert_eq!(
            click_count(Some(first), late, at, MouseButton::Left),
            1,
            "too late"
        );
        let far = point(px(110.0), px(50.0));
        assert_eq!(
            click_count(Some(first), soon, far, MouseButton::Left),
            1,
            "too far"
        );
    }
}
