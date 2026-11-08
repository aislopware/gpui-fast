//! Tests of the system's accessibility display options as the app holds them
//! ([`crate::App::accessibility_display`]).

use crate::{AccessibilityDisplay, TestAppContext};
use std::{cell::Cell, rc::Rc};

#[crate::test]
fn a_change_reaches_observers_once_and_a_repeat_reaches_none(cx: &mut TestAppContext) {
    let calls = Rc::new(Cell::new(0));
    let _subscription = cx.update(|cx| {
        cx.observe_accessibility_display({
            let calls = calls.clone();
            move |_| calls.set(calls.get() + 1)
        })
    });
    cx.update(|cx| {
        assert_eq!(cx.accessibility_display(), AccessibilityDisplay::default());
        cx.set_increase_contrast(true);
        assert!(cx.increase_contrast() && !cx.reduce_motion() && !cx.reduce_transparency());
        cx.set_increase_contrast(true);
        cx.set_reduce_motion(true);
        assert!(cx.reduce_motion() && cx.increase_contrast());
    });
    assert_eq!(calls.get(), 2);
}
