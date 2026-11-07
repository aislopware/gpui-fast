//! Setups that run for every [`App`] as it is made, test apps included: a library's globals and
//! key bindings, which its elements need wherever they are drawn. A component library registers
//! its own with [`on_every_app!`](crate::on_every_app), so no application or test can draw its
//! elements before it ran, and none has to remember to call it.
use crate::App;

/// One registered setup. Built by [`on_every_app!`](crate::on_every_app).
#[doc(hidden)]
pub struct AppInit(pub fn(&mut App));

inventory::collect!(AppInit);

/// Runs every registered setup on the App just made, in no particular order, before any window
/// opens.
pub(crate) fn run(cx: &mut App) {
    for init in inventory::iter::<AppInit> {
        (init.0)(cx);
    }
}

/// Runs `init` for every [`App`] as it is made, test apps included.
#[macro_export]
macro_rules! on_every_app {
    ($init:path) => {
        $crate::private::inventory::submit! { $crate::AppInit($init) }
    };
}

#[cfg(test)]
mod tests {
    use crate::{self as gpui, App, Global, TestAppContext};

    struct SetUp;

    impl Global for SetUp {}

    fn set_up(cx: &mut App) {
        cx.set_global(SetUp);
    }

    crate::on_every_app!(set_up);

    #[gpui::test]
    fn a_registered_setup_runs_before_a_test_app_is_used(cx: &mut TestAppContext) {
        cx.update(|cx| assert!(cx.has_global::<SetUp>()));
    }
}
