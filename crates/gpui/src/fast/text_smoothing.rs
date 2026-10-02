//! Whether glyphs are thickened by their colour: [`TextSmoothing`].
//!
//! With font smoothing on, Core Graphics dilates a glyph by the luminance of
//! the colour it is drawn in, so white text on black paints about a weight
//! heavier than black text on white. GPUI copies that on macOS
//! ([`crate::PlatformTextSystem::glyph_dilation_for_color`]), which matches
//! AppKit's own text. An application whose design is set in weights, as on
//! the web with `-webkit-font-smoothing: antialiased` or in Ghostty with
//! `font-thicken = false`, wants every colour drawn at the weight it is set
//! in.
//!
//! The policy is the application's ([`App::set_text_smoothing`]), and an
//! element can paint under another ([`Window::with_text_smoothing`]). It
//! decides the dilation a glyph is rasterised with, and the dilation is part
//! of the glyph's atlas key, so glyphs drawn under the two policies never
//! share a raster: an antialiased glyph in any colour is the raster a dark
//! glyph has under the platform's smoothing.

use crate::{App, Hsla, Window};
use std::cell::Cell;
use std::rc::Rc;

/// How glyphs are smoothed. See [`App::set_text_smoothing`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextSmoothing {
    /// As the platform smooths text: on macOS, unless font smoothing is
    /// switched off in the user's defaults, a glyph is dilated by the
    /// luminance of its colour, as Core Graphics draws AppKit's text. Other
    /// platforms dilate nothing.
    #[default]
    Native,
    /// Antialiased outlines that are never dilated: every colour paints the
    /// weight the font was set in.
    Antialiased,
}

/// The application's [`TextSmoothing`], shared with every window it opens.
#[derive(Default)]
pub(crate) struct AppTextSmoothing(Rc<Cell<TextSmoothing>>);

/// A window's view of the application's [`TextSmoothing`], and the policy an
/// element painting now chose with [`Window::with_text_smoothing`].
pub(crate) struct WindowTextSmoothing {
    app: Rc<Cell<TextSmoothing>>,
    scoped: Option<TextSmoothing>,
}

impl WindowTextSmoothing {
    pub(crate) fn new(cx: &App) -> Self {
        Self {
            app: cx.fast_text_smoothing.0.clone(),
            scoped: None,
        }
    }

    fn current(&self) -> TextSmoothing {
        self.scoped.unwrap_or_else(|| self.app.get())
    }
}

/// The dilation a glyph painted now in `color` is rasterised with.
#[inline]
pub(crate) fn dilation(window: &Window, color: Hsla) -> u8 {
    match window.fast_text_smoothing.current() {
        TextSmoothing::Native => window.text_system().glyph_dilation_for_color(color),
        TextSmoothing::Antialiased => 0,
    }
}

impl App {
    /// Sets how every window smooths its text, the ones open now and the ones
    /// opened later. Changing it draws every window again from scratch, as
    /// what windows kept from earlier frames was drawn under the old policy.
    pub fn set_text_smoothing(&mut self, smoothing: TextSmoothing) {
        if self.fast_text_smoothing.0.replace(smoothing) != smoothing {
            self.refresh_windows();
        }
    }

    /// How windows smooth their text. See [`App::set_text_smoothing`].
    pub fn text_smoothing(&self) -> TextSmoothing {
        self.fast_text_smoothing.0.get()
    }
}

impl Window {
    /// Paints what `f` paints with its text smoothed by `smoothing`, rather
    /// than by the application's policy ([`App::set_text_smoothing`]): a
    /// terminal following its own setting, say.
    ///
    /// What [`Window::paint_keyed`] draws again from the last frame is not
    /// painted afresh, so a key painted inside must change with `smoothing`.
    pub fn with_text_smoothing<R>(
        &mut self,
        smoothing: TextSmoothing,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let outer = self.fast_text_smoothing.scoped.replace(smoothing);
        let result = f(self);
        self.fast_text_smoothing.scoped = outer;
        result
    }

    /// How text painted now is smoothed: the policy of the innermost
    /// [`Window::with_text_smoothing`], or else the application's.
    pub fn text_smoothing(&self) -> TextSmoothing {
        self.fast_text_smoothing.current()
    }
}
