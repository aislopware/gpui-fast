//! The window's text style stack, which remembers the styles it resolves.

use crate::{TextStyle, TextStyleRefinement, Window};
use refineable::Refineable;
use std::{cell::RefCell, rc::Rc};

/// The text style refinements pushed while drawing, as `Window` keeps them,
/// and the text styles they resolve to.
///
/// Every text element asks for the text style in effect where it is, which
/// refines the default style by every refinement on the stack, and gets a
/// copy of its own. Elements under the same refinements, like the cells of a
/// table, all ask for the same style, so the style resolved at each depth is
/// kept until the stack changes there, and handed out as an `Rc`.
pub(crate) struct TextStyleStack {
    refinements: Vec<TextStyleRefinement>,
    /// `resolved[i]`, when known, is the default text style refined by the
    /// first `i` refinements. It always holds one more entry than
    /// `refinements`.
    resolved: RefCell<Vec<Option<Rc<TextStyle>>>>,
}

impl Default for TextStyleStack {
    fn default() -> Self {
        Self {
            refinements: Vec::new(),
            resolved: RefCell::new(vec![None]),
        }
    }
}

impl Clone for TextStyleStack {
    fn clone(&self) -> Self {
        Self {
            refinements: self.refinements.clone(),
            resolved: self.resolved.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.refinements.clone_from(&source.refinements);
        self.resolved
            .get_mut()
            .clone_from(&source.resolved.borrow());
    }
}

impl TextStyleStack {
    pub(crate) fn push(&mut self, refinement: TextStyleRefinement) {
        self.refinements.push(refinement);
        self.resolved.get_mut().push(None);
    }

    pub(crate) fn pop(&mut self) -> Option<TextStyleRefinement> {
        let refinement = self.refinements.pop()?;
        self.resolved.get_mut().pop();
        Some(refinement)
    }

    pub(crate) fn clear(&mut self) {
        self.refinements.clear();
        self.resolved.get_mut().truncate(1);
    }

    /// The default text style refined by every refinement on the stack, as
    /// [`Window::text_style`] returns it.
    pub(crate) fn resolve(&self) -> Rc<TextStyle> {
        let mut resolved = self.resolved.borrow_mut();
        let depth = self.refinements.len();
        if let Some(style) = &resolved[depth] {
            return style.clone();
        }
        let (mut style, from) = match resolved[..depth].iter().rposition(|style| style.is_some()) {
            Some(known) => ((**resolved[known].as_ref().unwrap()).clone(), known),
            None => (TextStyle::default(), 0),
        };
        for refinement in &self.refinements[from..] {
            style.refine(refinement);
        }
        let style = Rc::new(style);
        resolved[depth] = Some(style.clone());
        style
    }
}

/// The text style in effect, as [`Window::text_style`] returns it, without
/// copying it.
#[inline]
pub(crate) fn text_style(window: &Window) -> Rc<TextStyle> {
    window.text_style_stack.resolve()
}
