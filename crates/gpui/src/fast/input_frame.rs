//! The frame an input needs drawn before it is dispatched, drawn in an update
//! of its own.
//!
//! Two frames are drawn while an event is dispatched: a key draws the window
//! first when it is dirty (upstream's `dispatch_key_event`), as the first key
//! after the pointer always leaves it, and a press, or a key while the focus is
//! inside a scrolled layer's content, has that content painted again at the
//! offset shown ([`crate::fast::layers::input::before_dispatch`]). Drawn inside
//! the update that dispatches the event, such a frame's views queue their work
//! for after it (`Window::defer`, `Context::defer`) into the same effect cycle
//! as the event's handlers, so the work runs after them: a view that, once its
//! frame is drawn, gives the keyboard back to whatever that frame mounts finds
//! the focus a handler just moved to an element no frame has mounted yet, and
//! takes it away.
//!
//! [`prepare_input`] draws the frame in an update of its own before the update
//! that dispatches the event: its effects are flushed, its deferred work run,
//! before any handler sees the event, and the dispatch finds nothing left to
//! draw.

use crate::{
    AnyWindowHandle, AppContext, KeyDownEvent, Keystroke, PlatformInput, window::InputModality,
};

/// Draws, in an update of its own, the frame `event` needs before it is
/// dispatched to `window`, if it needs one, and runs the work that frame's
/// views deferred. Call it before the update that dispatches `event`, as the
/// platform's input does; a window that is gone is left alone.
pub fn prepare_input(window: AnyWindowHandle, event: &PlatformInput, cx: &mut impl AppContext) {
    cx.update_window(window, |_, window, cx| {
        // A key draws the window first when it is dirty (`dispatch_key_event`), and the first key
        // after the pointer dirties it, taking the keyboard's modality (`dispatch_event`).
        if event.keyboard_event().is_some() {
            if matches!(event, PlatformInput::KeyDown(_))
                && window.last_input_modality != InputModality::Keyboard
            {
                window.last_input_modality = InputModality::Keyboard;
                window.refresh();
            }
            if window.invalidator.is_dirty() {
                window.draw(cx).clear(cx);
            }
        }
        crate::fast::layers::input::draw_for(window, cx, event);
    })
    .ok();
}

/// [`prepare_input`] for `keystroke` pressed, as `Window::dispatch_keystroke`
/// dispatches it.
pub fn prepare_keystroke(window: AnyWindowHandle, keystroke: &Keystroke, cx: &mut impl AppContext) {
    let event = PlatformInput::KeyDown(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    prepare_input(window, &event, cx);
}
