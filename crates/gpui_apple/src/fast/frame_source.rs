//! Which source paces a Mac window's frames, as `gpui_macos` asks it.
//!
//! A `CVDisplayLink` tick that draws into the layer's `nextDrawable` reaches the glass sooner
//! than a `CAMetalDisplayLink` update does. The link schedules each update's drawable for the
//! third refresh after the update (its `targetPresentationTimestamp`), whatever
//! `preferredFrameLatency` asks, where a tick's frame shows at the first refresh its
//! compositor deadline allows; `SLOPTY.md` ("In the 0373366f sync") has the numbers. On some
//! Macs, though, `nextDrawable` blocks for two refreshes on every frame (base M4 and M5 chips
//! under macOS 26 and later, zed #65472), which holds a window to half the display's rate.
//!
//! So a window starts on the `CVDisplayLink`. Once `nextDrawable` has blocked for more than a
//! refresh on [`BLOCKED_OF_RECENT`] of the last eight frames, every window of the process
//! moves to its `CAMetalDisplayLink` (macOS 14 and later) and stays there.
//! `GPUI_METAL_DISPLAY_LINK=1` starts them there, and `GPUI_METAL_DISPLAY_LINK=0` keeps them
//! off it.

use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Of the last eight frames, how many waited more than a refresh for their drawable before
/// the windows move to the `CAMetalDisplayLink`. Not all eight, so that a frame now and then
/// whose drawable comes at once does not hide the stall; well over half, so that a burst of
/// slow frames on a busy machine does not move them.
const BLOCKED_OF_RECENT: u32 = 6;

/// `nextDrawable` was found blocking: Mac windows take their frames from the
/// `CAMetalDisplayLink` from now on.
static BLOCKED: AtomicBool = AtomicBool::new(false);

/// Whether Mac windows take their frames from a `CAMetalDisplayLink` rather than a
/// `CVDisplayLink`.
pub fn metal_link() -> bool {
    asked().unwrap_or_else(|| BLOCKED.load(Ordering::Relaxed))
}

/// What `GPUI_METAL_DISPLAY_LINK` asks: the link from the start, or never.
fn asked() -> Option<bool> {
    static ASKED: OnceLock<Option<bool>> = OnceLock::new();
    *ASKED.get_or_init(
        || match std::env::var("GPUI_METAL_DISPLAY_LINK").as_deref() {
            Ok("1") => Some(true),
            Ok("0") => Some(false),
            _ => None,
        },
    )
}

/// A renderer's last eight waits for the layer's next drawable, a bit each, set for a wait of
/// more than a refresh.
#[derive(Debug, Default)]
pub(crate) struct DrawableWaits {
    recent: u8,
}

/// The renderer waited `waited` for the layer's next drawable, on a display that refreshes
/// every `refresh`.
pub(crate) fn waited(waits: &mut DrawableWaits, waited: Duration, refresh: Duration) {
    if blocking(waits, waited, refresh) && !BLOCKED.swap(true, Ordering::Relaxed) {
        log::info!(
            "nextDrawable blocked for more than a refresh on {BLOCKED_OF_RECENT} of the last \
             8 frames: Mac windows move to CAMetalDisplayLink"
        );
    }
}

/// Takes in a wait, and says whether `nextDrawable` now blocks.
fn blocking(waits: &mut DrawableWaits, waited: Duration, refresh: Duration) -> bool {
    waits.recent = (waits.recent << 1) | u8::from(waited > refresh);
    waits.recent.count_ones() >= BLOCKED_OF_RECENT
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{BLOCKED_OF_RECENT, DrawableWaits, blocking};

    const REFRESH: Duration = Duration::from_micros(16_667);
    const STALL: Duration = Duration::from_millis(33);

    #[test]
    fn a_drawable_within_a_refresh_never_counts() {
        let mut waits = DrawableWaits::default();
        for _ in 0..64 {
            assert!(!blocking(&mut waits, REFRESH, REFRESH));
        }
    }

    #[test]
    fn a_stall_on_every_frame_counts_by_the_sixth() {
        let mut waits = DrawableWaits::default();
        for frame in 1..=BLOCKED_OF_RECENT {
            assert_eq!(
                blocking(&mut waits, STALL, REFRESH),
                frame == BLOCKED_OF_RECENT,
                "frame {frame}"
            );
        }
    }

    #[test]
    fn stalls_on_every_other_frame_do_not_count() {
        let mut waits = DrawableWaits::default();
        for frame in 0..64 {
            let waited = if frame % 2 == 0 {
                Duration::ZERO
            } else {
                STALL
            };
            assert!(!blocking(&mut waits, waited, REFRESH), "frame {frame}");
        }
    }

    #[test]
    fn stalls_on_six_of_eight_count_through_a_fast_frame_between() {
        let mut waits = DrawableWaits::default();
        let frames = [
            STALL,
            STALL,
            Duration::ZERO,
            STALL,
            STALL,
            Duration::ZERO,
            STALL,
        ];
        for waited in frames {
            assert!(!blocking(&mut waits, waited, REFRESH));
        }
        assert!(blocking(&mut waits, STALL, REFRESH));
    }
}
