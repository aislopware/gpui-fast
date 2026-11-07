//! A focused scroll container scrolls from the keyboard, as a focused
//! scroller does in a browser and an NSScrollView does on a Mac.
//!
//! Upstream scrolls a container only by the wheel and the trackpad. Base UI's
//! ScrollArea gets the browser's keys for free; here a container that tracks
//! focus and scrolls takes them while it holds the focus itself (not while a
//! child does, which may want the same keys for its own text or list): the
//! arrows by a line, Page Up/Down, Space and Shift-Space and Option-arrows by
//! a page less a line of overlap, Home/End and Command-arrows to either end.
//! A key that cannot move the container goes on to the bindings around it.

use crate::{
    DispatchPhase, Hitbox, Interactivity, KeyDownEvent, Keystroke, Overflow, Pixels, Point, Style,
    Window, point, px,
};

/// How far an arrow key scrolls, Safari's and AppKit's line.
pub(crate) const LINE: Pixels = px(40.);

/// How far `keystroke` asks to scroll a container whose viewport is `page`
/// along the axis it names, with `max` the farthest it scrolls; `None` for a
/// key that does not scroll. Offsets are negative, as the container keeps them:
/// a step down is a negative `y`.
pub(crate) fn step(
    keystroke: &Keystroke,
    page: Point<Pixels>,
    max: Point<Pixels>,
) -> Option<Point<Pixels>> {
    let m = &keystroke.modifiers;
    if m.control || m.function {
        return None;
    }
    let page_y = (page.y - LINE).max(LINE);
    let far_y = max.y + px(1.);
    let plain = !m.alt && !m.platform && !m.shift;
    Some(match keystroke.key.as_str() {
        "down" if plain => point(px(0.), -LINE),
        "up" if plain => point(px(0.), LINE),
        "right" if plain => point(-LINE, px(0.)),
        "left" if plain => point(LINE, px(0.)),
        "down" if m.alt && !m.platform => point(px(0.), -page_y),
        "up" if m.alt && !m.platform => point(px(0.), page_y),
        "down" if m.platform => point(px(0.), -far_y),
        "up" if m.platform => point(px(0.), far_y),
        "pagedown" if !m.alt && !m.platform => point(px(0.), -page_y),
        "pageup" if !m.alt && !m.platform => point(px(0.), page_y),
        "space" if !m.alt && !m.platform && !m.shift => point(px(0.), -page_y),
        "space" if m.shift && !m.alt && !m.platform => point(px(0.), page_y),
        "end" if !m.alt && !m.platform && !m.shift => point(px(0.), -far_y),
        "home" if !m.alt && !m.platform && !m.shift => point(px(0.), far_y),
        _ => return None,
    })
}

/// Where an `offset` lands after `delta` along the axes `overflow` lets it
/// scroll, kept within `max`.
pub(crate) fn land(
    offset: Point<Pixels>,
    delta: Point<Pixels>,
    max: Point<Pixels>,
    overflow: Point<Overflow>,
) -> Point<Pixels> {
    let axis = |offset: Pixels, delta: Pixels, max: Pixels, overflow: Overflow| {
        if overflow == Overflow::Scroll {
            (offset + delta).clamp(-max, px(0.))
        } else {
            offset
        }
    };
    point(
        axis(offset.x, delta.x, max.x, overflow.x),
        axis(offset.y, delta.y, max.y, overflow.y),
    )
}

/// Listens, for a container that tracks focus and scrolls, for the keys that
/// scroll it while it holds the focus.
pub(crate) fn listen(element: &Interactivity, hitbox: &Hitbox, style: &Style, window: &mut Window) {
    let (Some(focus), Some(offset)) = (
        element.tracked_focus_handle.clone(),
        element.scroll_offset.clone(),
    ) else {
        return;
    };
    let overflow = style.overflow;
    if overflow.x != Overflow::Scroll && overflow.y != Overflow::Scroll {
        return;
    }
    let max = crate::fast::layers::wheel::scroll_max(element, hitbox.bounds, style, window);
    let page = point(hitbox.bounds.size.width, hitbox.bounds.size.height);
    let current_view = window.current_view();
    let subtrees = crate::fast::retained::enclosing_retained_subtrees(window);
    let container = crate::fast::layers::invalidate::painted_container(window, element);
    window.on_key_event(move |event: &KeyDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || !focus.is_focused(window) {
            return;
        }
        let Some(delta) = step(&event.keystroke, page, max) else {
            return;
        };
        let mut offset = offset.borrow_mut();
        let landed = land(*offset, delta, max, overflow);
        if landed == *offset {
            return;
        }
        *offset = landed;
        crate::fast::retained::invalidate_retained_subtrees(window, &subtrees);
        crate::fast::layers::invalidate::note_scrolled(window, &container);
        cx.notify(current_view);
        cx.stop_propagation();
    });
}

#[cfg(test)]
mod tests {
    use super::{LINE, land, step};
    use crate::{Keystroke, Overflow, point, px};

    fn key(text: &'static str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    const PAGE: crate::Point<crate::Pixels> = point(px(300.), px(400.));
    const MAX: crate::Point<crate::Pixels> = point(px(0.), px(2000.));
    const Y: crate::Point<Overflow> = point(Overflow::Hidden, Overflow::Scroll);

    #[test]
    fn each_key_scrolls_its_distance() {
        assert_eq!(step(&key("down"), PAGE, MAX), Some(point(px(0.), -LINE)));
        assert_eq!(step(&key("up"), PAGE, MAX), Some(point(px(0.), LINE)));
        assert_eq!(
            step(&key("pagedown"), PAGE, MAX),
            Some(point(px(0.), px(-360.)))
        );
        assert_eq!(
            step(&key("space"), PAGE, MAX),
            Some(point(px(0.), px(-360.)))
        );
        assert_eq!(
            step(&key("shift-space"), PAGE, MAX),
            Some(point(px(0.), px(360.)))
        );
        assert_eq!(
            step(&key("alt-down"), PAGE, MAX),
            Some(point(px(0.), px(-360.)))
        );
        assert!(step(&key("cmd-down"), PAGE, MAX).unwrap().y < -MAX.y);
        assert!(step(&key("end"), PAGE, MAX).unwrap().y < -MAX.y);
        assert!(step(&key("home"), PAGE, MAX).unwrap().y > MAX.y);
        assert_eq!(step(&key("a"), PAGE, MAX), None);
        assert_eq!(step(&key("ctrl-down"), PAGE, MAX), None);
    }

    #[test]
    fn a_step_stays_within_the_ends_and_the_axes_that_scroll() {
        let top = point(px(0.), px(0.));
        assert_eq!(land(top, point(px(0.), LINE), MAX, Y), top);
        assert_eq!(
            land(top, point(px(0.), px(-5000.)), MAX, Y),
            point(px(0.), px(-2000.))
        );
        assert_eq!(land(top, point(-LINE, -LINE), MAX, Y), point(px(0.), -LINE));
    }
}
