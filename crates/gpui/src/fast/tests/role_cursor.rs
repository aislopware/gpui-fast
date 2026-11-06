//! Tests of the pointing hand over a control by its role. See
//! [`crate::fast::role_cursor`].

use crate::{
    Context, CursorStyle, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
    Render, StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext,
    Window, div, point, px,
};

/// Three rows of 100 × 40 at the window's top left: a button marked only by
/// its role, a link that names its own cursor, and a label.
struct Controls;

impl Render for Controls {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let row = |id: &'static str| div().id(id).w(px(100.)).h(px(40.));
        div()
            .size_full()
            .child(row("button").role(accesskit::Role::Button))
            .child(
                row("link")
                    .role(accesskit::Role::Link)
                    .cursor(CursorStyle::IBeam),
            )
            .child(row("label").role(accesskit::Role::Label))
    }
}

fn controls(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let (_, cx) = cx.add_window_view(|_, _| Controls);
    cx.update(|window, _| {
        window.activate_window();
        // Linux and Windows set the cursor only in the window the platform
        // reports hovered, which the test window never does.
        window.hovered.set(true);
    });
    cx.run_until_parked();
    cx
}

fn cursor_at(cx: &mut VisualTestContext, y: f32) -> CursorStyle {
    cx.simulate_mouse_move(point(px(50.), px(y)), None, Modifiers::default());
    *cx.test_platform.active_cursor.lock()
}

#[crate::test]
fn a_control_marked_only_by_its_role_shows_the_pointing_hand(cx: &mut TestAppContext) {
    let cx = controls(cx);
    assert_eq!(cursor_at(cx, 20.), CursorStyle::PointingHand, "the button");
    assert_eq!(
        cursor_at(cx, 60.),
        CursorStyle::IBeam,
        "its own cursor wins"
    );
    assert_eq!(
        cursor_at(cx, 100.),
        CursorStyle::Arrow,
        "a label is not clicked"
    );
    assert_eq!(cursor_at(cx, 200.), CursorStyle::Arrow, "outside every row");
    assert_eq!(
        cursor_at(cx, 20.),
        CursorStyle::PointingHand,
        "the button again"
    );
}

#[test]
fn only_roles_that_are_clicked_take_the_hand() {
    use accesskit::Role;
    for role in [
        Role::Button,
        Role::Link,
        Role::Tab,
        Role::ListBoxOption,
        Role::MenuItem,
        Role::MenuItemCheckBox,
        Role::MenuItemRadio,
        Role::CheckBox,
        Role::RadioButton,
        Role::Switch,
        Role::ComboBox,
        Role::TreeItem,
    ] {
        assert_eq!(
            crate::fast::role_cursor::of(Some(role)),
            Some(CursorStyle::PointingHand),
            "{role:?}"
        );
    }
    for role in [
        Role::Label,
        Role::Group,
        Role::TextInput,
        Role::Slider,
        Role::List,
    ] {
        assert_eq!(crate::fast::role_cursor::of(Some(role)), None, "{role:?}");
    }
    assert_eq!(crate::fast::role_cursor::of(None), None);
}
