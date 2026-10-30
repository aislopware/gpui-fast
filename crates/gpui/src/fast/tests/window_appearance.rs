//! Tests of a window's own appearance (`fast/window_appearance.rs`): it is
//! that window's alone, clearing it gives the window back the appearance it
//! followed, and the application's appearance, with whoever follows it, never
//! hears of it.

use std::{cell::RefCell, rc::Rc};

use crate::{
    self as gpui, Context, IntoElement, Render, TestAppContext, Window, WindowAppearance,
    WindowHandle, div,
};

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A window, and every appearance its observers were told it took.
fn observed_window(
    cx: &mut TestAppContext,
) -> (WindowHandle<Empty>, Rc<RefCell<Vec<WindowAppearance>>>) {
    let window = cx.add_window(|_, _| Empty);
    let told = Rc::new(RefCell::new(Vec::new()));
    window
        .update(cx, |_, window, _| {
            let told = told.clone();
            window
                .observe_window_appearance(move |window, _| {
                    told.borrow_mut().push(window.appearance())
                })
                .detach();
        })
        .unwrap();
    (window, told)
}

fn appearance(window: WindowHandle<Empty>, cx: &mut TestAppContext) -> WindowAppearance {
    window
        .update(cx, |_, window, _| window.appearance())
        .unwrap()
}

fn set(window: WindowHandle<Empty>, appearance: Option<WindowAppearance>, cx: &mut TestAppContext) {
    window
        .update(cx, |_, window, _| window.set_appearance(appearance))
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn a_windows_own_appearance_is_reported_by_it_alone(cx: &mut TestAppContext) {
    use WindowAppearance::{Dark, Light};
    let (own, own_told) = observed_window(cx);
    let (other, other_told) = observed_window(cx);

    set(own, Some(Dark), cx);
    assert_eq!(appearance(own, cx), Dark);
    assert_eq!(*own_told.borrow(), [Dark]);
    assert_eq!(
        appearance(other, cx),
        Light,
        "the other window keeps the app's"
    );
    assert!(
        other_told.borrow().is_empty(),
        "the other window hears nothing"
    );
    assert_eq!(
        cx.update(|cx| cx.window_appearance()),
        Light,
        "the app keeps its own"
    );

    // Asking again for what the window has already tells no one.
    set(own, Some(Dark), cx);
    assert_eq!(own_told.borrow().len(), 1);

    // The system turning dark turns the window that follows the app, and
    // tells only it: the other already wears dark, as its own.
    cx.simulate_system_appearance(Dark);
    cx.run_until_parked();
    assert_eq!(appearance(other, cx), Dark);
    assert_eq!(*other_told.borrow(), [Dark]);
    assert_eq!(own_told.borrow().len(), 1);

    // Its own light holds against the dark app, and cleared it is the app's
    // again.
    set(own, Some(Light), cx);
    assert_eq!(appearance(own, cx), Light);
    set(own, None, cx);
    assert_eq!(appearance(own, cx), Dark, "cleared, it follows the app's");
    assert_eq!(*own_told.borrow(), [Dark, Light, Dark]);
    assert_eq!(*other_told.borrow(), [Dark], "the other heard none of it");
}

#[gpui::test]
fn the_apps_appearance_is_told_apart_from_any_windows(cx: &mut TestAppContext) {
    let (window, _) = observed_window(cx);
    let told = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update(|cx| {
        let told = told.clone();
        cx.on_window_appearance_change(move |cx| told.borrow_mut().push(cx.window_appearance()))
    });

    set(window, Some(WindowAppearance::Dark), cx);
    assert!(
        told.borrow().is_empty(),
        "a window's own appearance is not the app's"
    );
    assert_eq!(
        cx.update(|cx| cx.window_appearance()),
        WindowAppearance::Light
    );

    // The system turning dark is told, though the only window has its own.
    cx.simulate_system_appearance(WindowAppearance::Dark);
    cx.run_until_parked();
    assert_eq!(*told.borrow(), [WindowAppearance::Dark]);

    // An appearance the app sets is told too, after the update that set it,
    // which has the app borrowed.
    cx.update(|cx| {
        cx.set_window_appearance(Some(WindowAppearance::Light));
        assert_eq!(told.borrow().len(), 1, "not while the app is borrowed");
    });
    cx.run_until_parked();
    assert_eq!(
        *told.borrow(),
        [WindowAppearance::Dark, WindowAppearance::Light]
    );

    // While the app has an appearance of its own, the system's is not its.
    cx.simulate_system_appearance(WindowAppearance::Light);
    cx.simulate_system_appearance(WindowAppearance::Dark);
    cx.run_until_parked();
    assert_eq!(told.borrow().len(), 2);
    assert_eq!(
        appearance(window, cx),
        WindowAppearance::Dark,
        "the window keeps its own throughout"
    );
}
