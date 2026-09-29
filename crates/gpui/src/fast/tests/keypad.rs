//! Tests that keypad keys can be bound on their own and still do what their
//! main twins do. See [`crate::fast::keypad`].

use crate::{
    AnyWindowHandle, AppContext as _, Context, FocusHandle, InputEvent as _,
    InteractiveElement as _, IntoElement, KeyBinding, KeyContext, KeyDownEvent, KeyUpEvent, Keymap,
    Keystroke, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, Window, div, px,
};
use std::{cell::RefCell, rc::Rc};

actions!(keypad_test, [Main, Keypad]);

fn bound(keymap: &Keymap, typed: &str) -> Vec<&'static str> {
    let (bindings, pending) = keymap.bindings_for_input(
        &[Keystroke::parse(typed).unwrap()],
        &[KeyContext::default()],
    );
    assert!(!pending, "{typed}");
    bindings
        .iter()
        .map(|binding| {
            if binding.action().partial_eq(&Main) {
                "main"
            } else {
                "keypad"
            }
        })
        .collect()
}

#[test]
fn a_keypad_key_matches_a_binding_for_its_main_twin() {
    let mut keymap = Keymap::default();
    keymap.add_bindings([
        KeyBinding::new("5", Main, None),
        KeyBinding::new("cmd-+", Main, None),
        KeyBinding::new("enter", Main, None),
        KeyBinding::new("shift-enter", Main, None),
    ]);
    for typed in ["kp5", "5", "cmd-kpadd", "kpenter", "enter", "shift-kpenter"] {
        assert_eq!(bound(&keymap, typed), ["main"], "{typed}");
    }
    // The modifiers still have to agree.
    for typed in ["cmd-kp5", "ctrl-kpenter", "kpsubtract", "kpequal"] {
        assert!(bound(&keymap, typed).is_empty(), "{typed}");
    }
}

#[test]
fn a_binding_for_a_keypad_key_leaves_the_main_key_alone() {
    let mut keymap = Keymap::default();
    keymap.add_bindings([
        KeyBinding::new("enter", Main, None),
        KeyBinding::new("kpenter", Keypad, None),
        KeyBinding::new("cmd-kp1", Keypad, None),
    ]);
    // The keypad's own binding was added later, so it comes first.
    assert_eq!(bound(&keymap, "kpenter"), ["keypad", "main"]);
    assert_eq!(bound(&keymap, "enter"), ["main"]);
    assert_eq!(bound(&keymap, "cmd-kp1"), ["keypad"]);
    assert!(bound(&keymap, "cmd-1").is_empty());
}

#[test]
fn a_simulated_keypad_key_types_what_its_main_twin_types() {
    for (typed, key_char) in [
        ("kp0", Some("0")),
        ("kp9", Some("9")),
        ("kpadd", Some("+")),
        ("kpdecimal", Some(".")),
        ("kpenter", Some("\n")),
        ("cmd-kp1", None),
    ] {
        let keystroke = Keystroke::parse(typed).unwrap().with_simulated_ime();
        assert_eq!(keystroke.key_char.as_deref(), key_char, "{typed}");
    }
}

struct Button {
    focus: FocusHandle,
    clicks: Rc<RefCell<usize>>,
}

impl Render for Button {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div().child(
            div()
                .id("button")
                .w(px(50.))
                .h(px(50.))
                .track_focus(&self.focus)
                .on_click(move |_, _, _| *clicks.borrow_mut() += 1),
        )
    }
}

fn press(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
    let keystroke = Keystroke::parse(key).unwrap();
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
    })
    .unwrap();
}

#[test]
fn the_keypad_enter_clicks_a_focused_element_as_enter_does() {
    let mut cx = TestAppContext::single();
    let focus = cx.update(|cx| cx.focus_handle());
    let clicks = Rc::new(RefCell::new(0));
    let window: AnyWindowHandle = cx
        .add_window({
            let focus = focus.clone();
            let clicks = clicks.clone();
            move |_, _| Button { focus, clicks }
        })
        .into();
    cx.update_window(window, |_, window, cx| window.focus(&focus, cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();

    press(&mut cx, window, "enter");
    assert_eq!(*clicks.borrow(), 1);
    press(&mut cx, window, "kpenter");
    assert_eq!(*clicks.borrow(), 2);
    press(&mut cx, window, "kp5");
    assert_eq!(*clicks.borrow(), 2);
}
