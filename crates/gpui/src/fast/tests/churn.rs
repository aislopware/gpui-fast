//! Debug builds' churn warnings ([`crate::fast::churn`]): a view notified on
//! every frame for nothing it shows is named once it has been built on
//! [`crate::fast::churn::UNCHANGED`] frames that drew the same; a view whose
//! notifies change what it shows is named only once it has been built on
//! [`crate::fast::churn::STREAK`] frames in a row; neither is named while the
//! warnings are off.

use std::cell::Cell;
use std::rc::Rc;

use crate::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, TestAppContext, Window, WindowHandle, div, hsla, px,
};

/// A label showing `shown`, as text and as a bar that long: the test
/// platform draws no glyphs, so the bar is what changes its scene.
struct Label {
    shown: Rc<Cell<u32>>,
}

impl Render for Label {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown.get();
        div()
            .h(px(20.))
            .w(px(10. + shown as f32))
            .bg(hsla(0.6, 0.5, 0.5, 1.))
            .child(SharedString::from(format!("{shown}")))
    }
}

struct Root {
    label: Entity<Label>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.label.clone())
    }
}

fn open(cx: &mut TestAppContext, on: bool) -> (WindowHandle<Root>, Entity<Label>, Rc<Cell<u32>>) {
    let shown = Rc::new(Cell::new(0));
    let label = cx.new({
        let shown = shown.clone();
        |_| Label { shown }
    });
    let window = cx.add_window({
        let label = label.clone();
        move |_, _| Root { label }
    });
    window
        .update(cx, |_, window, cx| {
            window.set_churn_warnings(on);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    (window, label, shown)
}

/// Notifies the label `frames` times, changing what it shows when `change`;
/// the warnings that name it.
fn run(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    label: &Entity<Label>,
    shown: &Rc<Cell<u32>>,
    frames: u32,
    change: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for _ in 0..frames {
        if change {
            shown.set(shown.get() + 1);
        }
        // A test window is drawn as the notify's update ends.
        label.update(cx, |_, cx| cx.notify());
        warnings.extend(
            window
                .update(cx, |_, window, _| window.take_churn_warnings())
                .unwrap(),
        );
    }
    warnings.retain(|warning| warning.contains("Label"));
    warnings
}

#[crate::test]
fn a_view_notified_for_nothing_it_shows_is_named_once(cx: &mut TestAppContext) {
    let (window, label, shown) = open(cx, true);
    let early = run(
        cx,
        window,
        &label,
        &shown,
        crate::fast::churn::UNCHANGED - 1,
        false,
    );
    assert!(
        early.is_empty(),
        "not before {} frames: {early:?}",
        crate::fast::churn::UNCHANGED
    );
    let warned = run(cx, window, &label, &shown, 10, false);
    assert_eq!(warned.len(), 1, "named once: {warned:?}");
    assert!(
        warned[0].contains("Label") && warned[0].contains("drew exactly what"),
        "{}",
        warned[0]
    );
}

#[crate::test]
fn a_view_whose_notifies_show_is_named_only_for_a_long_streak(cx: &mut TestAppContext) {
    let (window, label, shown) = open(cx, true);
    let early = run(
        cx,
        window,
        &label,
        &shown,
        crate::fast::churn::STREAK - 1,
        true,
    );
    assert!(early.is_empty(), "{early:?}");
    let warned = run(cx, window, &label, &shown, 5, true);
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].contains("each of the last"), "{}", warned[0]);
}

#[crate::test]
fn no_warning_while_off(cx: &mut TestAppContext) {
    let (window, label, shown) = open(cx, false);
    let warned = run(
        cx,
        window,
        &label,
        &shown,
        crate::fast::churn::STREAK + 10,
        false,
    );
    assert!(warned.is_empty(), "{warned:?}");
}
