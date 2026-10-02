//! CSS font weights for the weights Core Text gives its faces.
//!
//! font-kit turns Core Text's weight trait into a CSS weight through a table
//! of its own, which puts every Medium face at 530 and Heavy at 780. CSS
//! matching then never reaches them: 500 asks for 500 or less before
//! anything heavier and gets Regular, and 800 gets Black. Apple's faces, and
//! others' as Core Text reports them, carry AppKit's `NSFontWeight` values,
//! so those values are the CSS weights 100 to 900.

/// Core Text's weight trait at each CSS weight from 100 to 900: AppKit's
/// `NSFontWeightUltraLight` to `NSFontWeightBlack`.
const CORE_TEXT_WEIGHTS: [f64; 9] = [-0.8, -0.6, -0.4, 0.0, 0.23, 0.3, 0.4, 0.56, 0.62];

/// The CSS weight, 100 to 900, of a face whose Core Text weight trait is
/// `core_text`, to the nearest whole weight: AppKit's weights exactly (as
/// read through an `f32` too), and linear between them.
pub fn css_weight(core_text: f64) -> f32 {
    let last = CORE_TEXT_WEIGHTS.len() - 1;
    let index = match CORE_TEXT_WEIGHTS
        .windows(2)
        .position(|pair| core_text < pair[1])
    {
        _ if core_text <= CORE_TEXT_WEIGHTS[0] => 0.,
        Some(below) => {
            let (low, high) = (CORE_TEXT_WEIGHTS[below], CORE_TEXT_WEIGHTS[below + 1]);
            below as f64 + (core_text - low) / (high - low)
        }
        None => last as f64,
    };
    (100. + 100. * index).round() as f32
}

#[cfg(test)]
mod tests {
    use super::{CORE_TEXT_WEIGHTS, css_weight};

    /// AppKit's weights are the CSS weights, Medium 500 and Heavy 800 among
    /// them; a weight between two is between their CSS weights, and one past
    /// either end is the end.
    #[test]
    fn appkit_weights_are_the_css_weights() {
        let css: Vec<f32> = CORE_TEXT_WEIGHTS.iter().map(|&w| css_weight(w)).collect();
        assert_eq!(css, [100., 200., 300., 400., 500., 600., 700., 800., 900.]);
        assert_eq!(css_weight(0.23_f32 as f64), 500., "a trait read as an f32");
        assert_eq!(css_weight(0.115), 450.);
        assert_eq!(css_weight(-1.), 100.);
        assert_eq!(css_weight(1.), 900.);
    }
}
