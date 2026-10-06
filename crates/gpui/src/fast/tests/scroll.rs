//! Tests of momentum told apart from the scrolling a finger drives.

use std::time::{Duration, Instant};

use crate::{
    GestureTuning, RecognizedTouchGesture, TouchEvent, TouchGestureRecognizer, TouchId, TouchPhase,
    point, px,
};

fn touch(id: u64, phase: TouchPhase, y: f32) -> TouchEvent {
    TouchEvent {
        id: TouchId(id),
        phase,
        position: point(px(100.), px(y)),
        predicted_position: None,
        force: None,
    }
}

/// The phases of the scrolls recognized: the touch's, and the momentum's.
fn phases(
    recognized: impl IntoIterator<Item = RecognizedTouchGesture>,
) -> Vec<(TouchPhase, Option<TouchPhase>)> {
    recognized
        .into_iter()
        .filter_map(|gesture| match gesture {
            RecognizedTouchGesture::Scroll(scroll) => {
                Some((scroll.touch_phase, scroll.momentum_phase))
            }
            _ => None,
        })
        .collect()
}

/// Swipes up from `y` and lets go at speed, from `at`.
fn fling(
    recognizer: &mut TouchGestureRecognizer,
    id: u64,
    at: Instant,
) -> Vec<(TouchPhase, Option<TouchPhase>)> {
    let mut recognized = Vec::new();
    for step in 0..=4u64 {
        let phase = match step {
            0 => TouchPhase::Started,
            4 => TouchPhase::Ended,
            _ => TouchPhase::Moved,
        };
        recognized.extend(recognizer.handle_event_at(
            &touch(id, phase, 300. - step as f32 * 33.),
            at + Duration::from_millis(step * 16),
        ));
    }
    phases(recognized)
}

#[test]
fn a_fling_is_momentum_from_its_first_step_to_its_last() {
    let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
    let start = Instant::now();
    let swipe = fling(&mut recognizer, 1, start);
    assert!(!swipe.is_empty());
    assert!(
        swipe.iter().all(|(_, momentum)| momentum.is_none()),
        "{swipe:?}"
    );

    let mut steps = Vec::new();
    let mut at = start + Duration::from_millis(64);
    while let Some(step) = recognizer.tick_momentum_at(at) {
        steps.extend(phases([step]));
        at += Duration::from_millis(16);
    }
    assert!(steps.len() > 2, "{steps:?}");
    assert_eq!(
        steps.first(),
        Some(&(TouchPhase::Moved, Some(TouchPhase::Started)))
    );
    assert_eq!(
        steps.last(),
        Some(&(TouchPhase::Ended, Some(TouchPhase::Ended)))
    );
    assert!(
        steps[1..steps.len() - 1]
            .iter()
            .all(|step| *step == (TouchPhase::Moved, Some(TouchPhase::Moved))),
        "{steps:?}"
    );
}

#[test]
fn a_touch_catching_a_fling_ends_its_momentum_and_drives_its_own_scroll() {
    let mut recognizer = TouchGestureRecognizer::new(GestureTuning::default());
    let start = Instant::now();
    fling(&mut recognizer, 1, start);
    let step = recognizer.tick_momentum_at(start + Duration::from_millis(80));
    assert_eq!(
        phases(step),
        [(TouchPhase::Moved, Some(TouchPhase::Started))]
    );

    let caught = recognizer.handle_event_at(
        &touch(2, TouchPhase::Started, 150.),
        start + Duration::from_millis(96),
    );
    assert_eq!(
        phases(caught),
        [
            (TouchPhase::Ended, Some(TouchPhase::Ended)),
            (TouchPhase::Started, None),
        ]
    );
}

/// A scroll handle's item brought into view against the frame it is asked in.
mod item_into_view {
    use crate::{
        AnyWindowHandle, AppContext, Context, InteractiveElement, IntoElement, ParentElement,
        Pixels, Render, ScrollHandle, StatefulInteractiveElement, Styled, TestAppContext, Window,
        div, px,
    };

    struct Row {
        width: Pixels,
        handle: ScrollHandle,
    }

    impl Render for Row {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("row")
                .w(self.width)
                .h(px(20.))
                .flex()
                .overflow_x_scroll()
                .track_scroll(&self.handle)
                .children((0..4).map(|_| div().flex_none().w(px(60.)).h(px(20.))))
        }
    }

    /// An item asked for in the frame a scroll container narrows is brought into view against
    /// the container's new width, not the width of the frame before, which would judge the
    /// item visible and drop the request.
    #[crate::test]
    fn scroll_to_item_uses_the_frames_own_bounds(cx: &mut TestAppContext) {
        let handle = ScrollHandle::new();
        let window = cx.add_window({
            let handle = handle.clone();
            move |_, _| Row {
                width: px(240.),
                handle,
            }
        });
        let any = AnyWindowHandle::from(window);
        cx.update_window(any, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(handle.offset().x, px(0.), "every item fits at 240");

        window
            .update(cx, |view, _, _| {
                view.width = px(100.);
                view.handle.scroll_to_item(3);
            })
            .unwrap();
        cx.update_window(any, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(
            handle.offset().x,
            px(-140.),
            "the last item's right edge on the row's"
        );
    }
}
