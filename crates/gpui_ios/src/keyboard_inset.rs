//! How much of a window the keyboard covers, from the frame UIKit reports for it.
//!
//! UIKit posts the frame the keyboard will have once it has moved
//! (`UIKeyboardFrameEndUserInfoKey`), in the screen's coordinate space. Its height is
//! not what the window loses:
//!
//! - With a hardware keyboard the software keyboard's frame is pushed below the screen,
//!   or up just far enough to show the shortcuts bar. Only the part on the screen covers
//!   anything.
//! - A floating, undocked or split keyboard moved up the screen floats over the content
//!   and reserves nothing, as UIKit's own keyboard layout guide treats it. UIKit reports
//!   it either as an empty frame or as one that stops short of the screen's bottom.
//! - A window that does not reach the bottom of the screen (Stage Manager, Slide Over,
//!   split view) loses only the part of it the keyboard overlaps, and nothing when the
//!   keyboard is below it.
//!
//! A docked keyboard spans the screen, so only its vertical extent is read. iPadOS 26
//! has been seen to report a shifted `x` for windowed apps.
//!
//! This module is pure and compiled on the host too, so its tests run there.

use gpui::{Bounds, Pixels, px};

/// How close to the screen's bottom a keyboard has to end to count as docked: frames
/// are fractional on screens whose scale is not a whole number.
const DOCKED_SLACK: Pixels = px(0.5);

/// How far up from its bottom edge `window` is covered by `keyboard`, a keyboard frame on
/// `screen`. All three are in one coordinate space; UIKit reports the keyboard's in the
/// screen's, and the window's view converts them to its own.
///
/// Zero for a keyboard that covers none of the window: one with an empty frame, one below
/// the screen or the window, and one floating rather than docked at the screen's bottom.
pub(crate) fn keyboard_inset(
    keyboard: Bounds<Pixels>,
    screen: Bounds<Pixels>,
    window: Bounds<Pixels>,
) -> Pixels {
    if keyboard.is_empty() || window.is_empty() {
        return Pixels::ZERO;
    }
    if keyboard.bottom() < screen.bottom() - DOCKED_SLACK {
        return Pixels::ZERO;
    }
    let top = keyboard.top().max(window.top());
    (window.bottom() - top)
        .max(Pixels::ZERO)
        .min(window.size.height)
}

#[cfg(test)]
mod tests {
    use super::keyboard_inset;
    use gpui::{Bounds, Pixels, bounds, point, px, size};

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        bounds(point(px(x), px(y)), size(px(width), px(height)))
    }

    /// An 11-inch iPad in landscape, in points.
    const WIDTH: f32 = 1194.;
    const HEIGHT: f32 = 834.;

    fn screen() -> Bounds<Pixels> {
        rect(0., 0., WIDTH, HEIGHT)
    }

    /// The software keyboard docked at the screen's bottom.
    fn docked(height: f32) -> Bounds<Pixels> {
        rect(0., HEIGHT - height, WIDTH, height)
    }

    #[test]
    fn a_docked_keyboard_covers_its_height_of_a_full_screen_window() {
        assert_eq!(keyboard_inset(docked(398.), screen(), screen()), px(398.));
    }

    #[test]
    fn a_keyboard_pushed_below_the_screen_by_a_hardware_keyboard_covers_nothing() {
        let below = rect(0., HEIGHT, WIDTH, 398.);
        assert_eq!(keyboard_inset(below, screen(), screen()), px(0.));
    }

    #[test]
    fn a_hardware_keyboard_s_shortcuts_bar_covers_only_what_shows_of_it() {
        // The software keyboard's frame, pushed down until only its bar is on the screen.
        let pushed = rect(0., HEIGHT - 55., WIDTH, 398.);
        assert_eq!(keyboard_inset(pushed, screen(), screen()), px(55.));
        // The bar alone, docked.
        assert_eq!(keyboard_inset(docked(55.), screen(), screen()), px(55.));
    }

    #[test]
    fn a_floating_keyboard_covers_nothing() {
        // Its own small frame, up the screen.
        let floating = rect(437., 300., 320., 260.);
        assert_eq!(keyboard_inset(floating, screen(), screen()), px(0.));
        // Or an empty one, as UIKit reports it while floating or dragged.
        assert_eq!(
            keyboard_inset(Bounds::default(), screen(), screen()),
            px(0.)
        );
    }

    #[test]
    fn an_undocked_or_split_keyboard_moved_up_covers_nothing() {
        let undocked = rect(0., 200., WIDTH, 330.);
        assert_eq!(keyboard_inset(undocked, screen(), screen()), px(0.));
    }

    #[test]
    fn a_split_keyboard_left_docked_covers_its_height() {
        // Its frame spans the screen with the gap between the halves; the band is lost.
        assert_eq!(keyboard_inset(docked(262.), screen(), screen()), px(262.));
    }

    #[test]
    fn a_stage_manager_window_loses_only_the_part_the_keyboard_overlaps() {
        let keyboard = docked(398.);
        // Reaching 100 points into the keyboard.
        let window = rect(200., 136., 700., 400.);
        assert_eq!(window.bottom(), keyboard.top() + px(100.));
        assert_eq!(keyboard_inset(keyboard, screen(), window), px(100.));
        // Wholly above it.
        let above = rect(200., 20., 700., 380.);
        assert_eq!(keyboard_inset(keyboard, screen(), above), px(0.));
        // Wholly over it: covered top to bottom, never more.
        let low = rect(200., 500., 700., 300.);
        assert_eq!(keyboard_inset(keyboard, screen(), low), px(300.));
    }

    #[test]
    fn a_shifted_x_does_not_hide_a_docked_keyboard() {
        // iPadOS 26 has reported docked keyboards of windowed apps with a negative `x`.
        let shifted = rect(-1500., HEIGHT - 398., WIDTH, 398.);
        let window = rect(600., 100., 500., 600.);
        assert_eq!(keyboard_inset(shifted, screen(), window), px(264.));
    }

    #[test]
    fn a_window_reads_the_same_in_its_own_coordinates() {
        // A window at (300, 100) on the screen, 600 points tall, ends 264 points into the
        // keyboard. Its view converts the screen and the keyboard into its own space, where
        // both move by (-300, -100).
        let on_screen = rect(300., 100., 600., 600.);
        assert_eq!(keyboard_inset(docked(398.), screen(), on_screen), px(264.));
        let window = rect(0., 0., 600., 600.);
        let screen = rect(-300., -100., WIDTH, HEIGHT);
        let keyboard = rect(-300., -100. + HEIGHT - 398., WIDTH, 398.);
        assert_eq!(keyboard_inset(keyboard, screen, window), px(264.));
    }

    #[test]
    fn a_fractional_frame_still_counts_as_docked() {
        let keyboard = rect(0., HEIGHT - 398.25, WIDTH, 398.);
        assert_eq!(keyboard_inset(keyboard, screen(), screen()), px(398.25));
    }
}
