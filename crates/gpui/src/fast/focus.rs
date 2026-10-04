//! What a retained subtree read of the window's focus.
//!
//! Upstream refreshes the whole window whenever the focus moves, which here
//! would build every view again. Instead, each question a view asks of the
//! focus through a [`FocusId`] — is it focused, does it contain the focused
//! element, is it within it — is recorded as a read of a [`StateVersion`]
//! kept for that handle and that question, with the answer the view saw.
//! Before a frame is drawn, every question still on record is asked again,
//! and the version of each whose answer changed is bumped, so only the views
//! whose answer changed are built again: focus moving from A to B builds the
//! views that asked about A or B, not one that asked about C.
//!
//! [`crate::Window::focused`] hands out the focused element itself, which
//! the view may compare with anything; reading it counts as reading the
//! focus as a whole, and any move of the focus builds the view again.
//!
//! A view may move the focus as it renders, as a workspace gives the
//! keyboard to the tile it was asked to. The questions are then asked again
//! at once, so the views drawn after it in that frame, drawn again from last
//! frame or not, show the focus as it is; the views drawn before it showed
//! it as it was, and the window asks for another frame for them once the
//! frame is drawn, if the focus is not where it was when the frame began.

use crate::fast::dependencies::{StateVersion, ambient};
use crate::{App, FocusId, Window};
use collections::FxHashMap;
use std::cell::{Cell, RefCell};

/// A question a view asks of the focus about one handle.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Question {
    /// [`FocusId::is_focused`].
    Focused,
    /// [`FocusId::contains_focused`].
    ContainsFocused,
    /// [`FocusId::within_focused`].
    WithinFocused,
}

/// The questions views asked of a window's focus, each with the version its
/// readers recorded and the answer they saw.
#[derive(Default)]
pub(crate) struct FocusReads {
    answers: RefCell<FxHashMap<(FocusId, Question), (StateVersion, bool)>>,
    /// The focus the questions on record were last asked again against.
    stamped: Cell<Option<FocusId>>,
    /// The focus when the frame being drawn began, while one is.
    frame_began: Cell<Option<Option<FocusId>>>,
}

/// [`FocusId::is_focused`], recorded.
pub(crate) fn is_focused(id: FocusId, window: &Window) -> bool {
    let answer = window.focus == Some(id);
    note(id, Question::Focused, answer, window);
    answer
}

/// [`FocusId::contains_focused`], recorded.
pub(crate) fn contains_focused(id: FocusId, window: &Window, cx: &App) -> bool {
    let answer = ask(id, Question::ContainsFocused, window, cx);
    note(id, Question::ContainsFocused, answer, window);
    answer
}

/// [`FocusId::within_focused`], recorded.
pub(crate) fn within_focused(id: FocusId, window: &Window, cx: &App) -> bool {
    let answer = ask(id, Question::WithinFocused, window, cx);
    note(id, Question::WithinFocused, answer, window);
    answer
}

/// Records, for any recording that is open, that the focus as a whole was
/// read, through [`crate::Window::focused`].
#[inline]
pub(crate) fn read_focused(window: &Window) {
    window.retained_state.ambient_reads.note::<ambient::Focus>();
}

/// What [`crate::Window::focus`] and [`crate::Window::blur`] do where
/// upstream refreshes the window: the views that read the focus as a whole
/// are built again, and the window is drawn, where the questions views asked
/// of it are asked again.
///
/// While the window draws, the questions are asked again now, and the views
/// whose answers changed marked, as before a frame, so that the views drawn
/// after this in the frame are built again rather than drawn from last
/// frame. The frame for the views drawn before is asked for once the frame
/// is drawn ([`finish_frame`]).
pub(crate) fn focus_changed(window: &mut Window, cx: &mut App) {
    cx.ambient_changed::<ambient::Focus>();
    if window.invalidator.not_drawing() {
        window.invalidator.set_dirty(true);
    } else if window.focus != window.retained_state.focus_reads.stamped.get() {
        stamp_changes(window, cx);
        window.mark_changed_retained_views_dirty(cx);
    }
}

/// Asks every question on record again as a frame begins. See
/// [`stamp_changes`].
pub(crate) fn begin_frame(window: &Window, cx: &App) {
    let reads = &window.retained_state.focus_reads;
    reads.frame_began.set(Some(window.focus));
    stamp_changes(window, cx);
}

/// Asks for another frame when the focus moved while this one was drawn: a
/// view drawn before it moved shows it as it was. Only a move counts, not
/// the focus being set: a view that blurs, or focuses what has the focus,
/// each time it is built must not keep its window drawing, nor one whose
/// render moves it away and back.
pub(crate) fn finish_frame(window: &Window) {
    let began = window.retained_state.focus_reads.frame_began.take();
    if began.is_some_and(|began| began != window.focus) {
        window.invalidator.set_dirty(true);
    }
}

/// Asks every question on record again, before a frame is drawn, bumping
/// the version of each whose answer changed. A question is kept while its
/// handle lives, not only while a retained subtree holds its version, so
/// that a subtree built every frame finds the same version, and so the same
/// interned list of what it read, instead of making both again.
pub(crate) fn stamp_changes(window: &Window, cx: &App) {
    let reads = &window.retained_state.focus_reads;
    reads.stamped.set(window.focus);
    let mut answers = reads.answers.borrow_mut();
    if answers.is_empty() {
        return;
    }
    let handles = cx.focus_handles.read();
    let alive = |id: FocusId| {
        handles
            .get(id)
            .is_some_and(|focus| focus.ref_count.load(std::sync::atomic::Ordering::SeqCst) > 0)
    };
    let focused = window.focus.filter(|focused| alive(*focused));
    answers.retain(|&(id, question), (version, answered)| {
        if !version.is_shared() && !alive(id) {
            return false;
        }
        let answer = answer(id, question, focused, window);
        if answer != *answered {
            version.bump();
            *answered = answer;
        }
        true
    });
}

/// The answer to `question` about `id`, as upstream gives it: the focused
/// element is the one the window focuses while its handle is alive. See
/// [`answer`] for which frame says what contains what.
fn ask(id: FocusId, question: Question, window: &Window, cx: &App) -> bool {
    let focused = window.focus.filter(|focused| {
        cx.focus_handles
            .read()
            .get(*focused)
            .is_some_and(|focus| focus.ref_count.load(std::sync::atomic::Ordering::SeqCst) > 0)
    });
    answer(id, question, focused, window)
}

/// The answer to `question` about `id` while `focused` has the focus.
///
/// What contains what is as the frame being painted has it while one is,
/// its dispatch tree whole since its prepaint, and as the last frame drawn
/// has it otherwise. Upstream asks the last frame drawn always, so an
/// element drawn for the first time inside the focused element is not
/// within it until a later frame, and nothing asks for that frame: the
/// focus did not move. Prepaint still asks the last frame drawn, the tree
/// being drawn not yet holding what comes after the element asking.
fn answer(id: FocusId, question: Question, focused: Option<FocusId>, window: &Window) -> bool {
    let tree = if window.invalidator.painting() {
        &window.next_frame.dispatch_tree
    } else {
        &window.rendered_frame.dispatch_tree
    };
    match question {
        Question::Focused => window.focus == Some(id),
        Question::ContainsFocused => {
            focused.is_some_and(|focused| tree.focus_contains(id, focused))
        }
        Question::WithinFocused => focused.is_some_and(|focused| tree.focus_contains(focused, id)),
    }
}

/// Records, for any recording that is open, that `question` about `id` was
/// answered `answer`. A reader seeing another answer than the one on record
/// sees the focus as it is now, which the readers before it did not: the
/// version is bumped for them.
fn note(id: FocusId, question: Question, answer: bool, window: &Window) {
    let reads = &window.retained_state.ambient_reads;
    if !reads.recording() {
        return;
    }
    let mut answers = window.retained_state.focus_reads.answers.borrow_mut();
    let (version, answered) = answers
        .entry((id, question))
        .or_insert_with(|| (StateVersion::default(), answer));
    if *answered != answer {
        version.bump();
        *answered = answer;
    }
    reads.note_state(version);
}
