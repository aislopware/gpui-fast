//! A focused scroll container scrolls from the keyboard; a focused child keeps
//! its keys. See [`crate::fast::scroll_keys`].

use crate::{
    AnyWindowHandle, AppContext as _, Context, FocusHandle, InputEvent as _,
    InteractiveElement as _, IntoElement, KeyDownEvent, Keystroke, ParentElement as _, Render,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, px,
};

/// A 400 pt tall scroller over 2000 pt of content, with a focusable child.
struct Scroller {
    focus: FocusHandle,
    child: FocusHandle,
    scroll: ScrollHandle,
}

impl Render for Scroller {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .id("scroller")
                .w(px(300.))
                .h(px(400.))
                .overflow_y_scroll()
                .track_focus(&self.focus)
                .track_scroll(&self.scroll)
                .child(div().id("child").h(px(20.)).track_focus(&self.child))
                .child(div().h(px(1980.))),
        )
    }
}

struct Setup {
    cx: TestAppContext,
    window: AnyWindowHandle,
    focus: FocusHandle,
    child: FocusHandle,
    scroll: ScrollHandle,
}

impl Setup {
    fn new() -> Self {
        let mut cx = TestAppContext::single();
        let (focus, child) = cx.update(|cx| (cx.focus_handle(), cx.focus_handle()));
        let scroll = ScrollHandle::new();
        let window = cx
            .add_window({
                let (focus, child, scroll) = (focus.clone(), child.clone(), scroll.clone());
                move |_, _| Scroller {
                    focus,
                    child,
                    scroll,
                }
            })
            .into();
        let mut setup = Self {
            cx,
            window,
            focus,
            child,
            scroll,
        };
        setup.draw();
        setup
    }

    fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn focus(&mut self, focus: &FocusHandle) {
        self.cx
            .update_window(self.window, |_, window, cx| window.focus(focus, cx))
            .unwrap();
        self.draw();
    }

    fn press(&mut self, key: &'static str) {
        let keystroke = Keystroke::parse(key).unwrap();
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    KeyDownEvent {
                        keystroke,
                        is_held: false,
                        prefer_character_input: false,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        self.draw();
    }

    fn offset(&self) -> f32 {
        self.scroll.offset().y.as_f32()
    }
}

#[test]
fn a_focused_scroller_takes_the_scrolling_keys() {
    let mut s = Setup::new();
    let focus = s.focus.clone();
    s.focus(&focus);
    s.press("down");
    assert_eq!(s.offset(), -40.);
    s.press("pagedown");
    assert_eq!(s.offset(), -400.);
    s.press("shift-space");
    assert_eq!(s.offset(), -40.);
    s.press("end");
    assert_eq!(s.offset(), -1600.);
    s.press("down");
    assert_eq!(s.offset(), -1600.);
    s.press("cmd-up");
    assert_eq!(s.offset(), 0.);
}

#[test]
fn a_focused_child_keeps_its_keys() {
    let mut s = Setup::new();
    let child = s.child.clone();
    s.focus(&child);
    s.press("pagedown");
    s.press("space");
    assert_eq!(s.offset(), 0.);
}
