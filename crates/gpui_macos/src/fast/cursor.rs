//! `CursorStyle::Image` on macOS: an `NSCursor` built from the application's
//! picture, registered as the view's cursor rect like every other style.
//!
//! The cursors live on the main thread, where AppKit asks for cursor rects.
//! Each id holds the cursor it shows, and the last [`BUILT_KEPT`] cursors
//! built are kept by the picture's content, so pointing an id back at a
//! picture it showed before builds nothing.
//!
//! Pointing an id at another picture invalidates the cursor rects of every
//! GPUI window whose view shows the id, so AppKit takes the new cursor for
//! them whenever the window is next key, and sets the cursor at once when the
//! pointer is over the key window's view, so the change does not wait for
//! AppKit to rebuild the rects or for the next frame.

use std::cell::RefCell;

use cocoa::appkit::NSApplication;
use cocoa::base::{id, nil};
use cocoa::foundation::{NSInteger, NSPoint, NSRect, NSUInteger};
use collections::HashMap;
use gpui::{CursorImage, CursorImageId, CursorStyle};
use objc::{class, msg_send, sel, sel_impl};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSCursor, NSImage};
use objc2_core_foundation::{CFData, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};

use crate::window::{get_window_state, is_gpui_window};

/// How many built cursors are kept for reuse beyond the ones ids show. Enough
/// for every shape an application shows in a session (arrow, I-beam, hand,
/// the resize arrows, a busy wheel's frames) several times over.
pub(crate) const BUILT_KEPT: usize = 64;

#[derive(Default)]
struct Cursors {
    /// The cursor each id shows, with its picture.
    shown: HashMap<CursorImageId, (CursorImage, Retained<NSCursor>)>,
    /// Cursors built with their pictures, least recently used first. A hit
    /// compares the whole picture, not only its key, so two pictures whose
    /// keys collide never share a cursor.
    built: Vec<(CursorImage, Retained<NSCursor>)>,
    /// How many cursors were built, for tests.
    builds: usize,
}

thread_local! {
    static CURSORS: RefCell<Cursors> = RefCell::default();
}

impl Cursors {
    fn cursor_for(&mut self, image: &CursorImage) -> Option<Retained<NSCursor>> {
        if let Some(at) = self.built.iter().position(|(built, _)| same(built, image)) {
            let entry = self.built.remove(at);
            let cursor = entry.1.clone();
            self.built.push(entry);
            return Some(cursor);
        }
        let cursor = build(image)?;
        self.builds += 1;
        if self.built.len() == BUILT_KEPT {
            self.built.remove(0);
        }
        self.built.push((image.clone(), cursor.clone()));
        Some(cursor)
    }

    /// Points `id` at `image`; whether the cursor `id` shows changed.
    fn set(&mut self, id: CursorImageId, image: Option<&CursorImage>) -> bool {
        let Some(image) = image else {
            return self.shown.remove(&id).is_some();
        };
        if self
            .shown
            .get(&id)
            .is_some_and(|(shown, _)| same(shown, image))
        {
            return false;
        }
        match self.cursor_for(image) {
            Some(cursor) => {
                self.shown.insert(id, (image.clone(), cursor));
                true
            }
            None => {
                log::error!("no cursor could be made of a {:?} picture", image.size());
                self.shown.remove(&id).is_some()
            }
        }
    }
}

/// Whether two pictures are the same: the keys first, which differ for almost
/// every pair, then the pixels, so a collision of keys is never taken for a
/// match.
fn same(a: &CursorImage, b: &CursorImage) -> bool {
    a.key() == b.key() && a == b
}

/// Builds the cursor for `image`: an `NSImage` sized in points over one
/// `CGImage` of the picture's pixels, so AppKit draws it pixel for pixel on a
/// display of the picture's scale, and the hotspot in points.
pub(crate) fn build(image: &CursorImage) -> Option<Retained<NSCursor>> {
    let size = image.size();
    let (width, height) = (size.width.0 as usize, size.height.0 as usize);
    let data = CFData::from_bytes(image.bgra());
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: `kCGColorSpaceSRGB` is an immutable CFString constant CoreGraphics
    // exports for the process's lifetime.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    // Premultiplied BGRA in memory is ARGB read as a little-endian 32-bit word.
    let info = CGBitmapInfo(
        CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
    );
    // SAFETY: a null decode array is CGImageCreate's documented "no decode";
    // the provider holds `width * height * 4` bytes, which `CursorImage::new`
    // checked, laid out as `bytes_per_row = width * 4` and the bitmap info say.
    let picture = unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            Some(&space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }?;
    let points = image.size_in_points();
    let ns_image = NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &picture,
        CGSize::new(points.width.into(), points.height.into()),
    );
    let hotspot = image.hotspot_in_points();
    // NSCursor's hotspot is in points from the top left, as the picture's is.
    Some(NSCursor::initWithImage_hotSpot(
        NSCursor::alloc(),
        &ns_image,
        objc2_core_foundation::CGPoint::new(hotspot.x.into(), hotspot.y.into()),
    ))
}

/// Points `id` at `image` (the platform's `set_cursor_image`), and shows the
/// new cursor at once when the key window shows `id`'s.
pub(crate) fn set_image(id: CursorImageId, image: Option<CursorImage>) {
    if CURSORS.with_borrow_mut(|cursors| cursors.set(id, image.as_ref())) {
        // SAFETY: the platform calls this on the AppKit main thread.
        unsafe { show_now(id) };
    }
}

/// The cursor for a view whose style is `Image(id)`, autoreleased for
/// `addCursorRect:cursor:`, which retains it: `id`'s picture, or the arrow when
/// `id` points at none.
///
/// # Safety
///
/// Must run on the AppKit main thread inside an autorelease pool, which is
/// where `resetCursorRects` is invoked.
pub(crate) unsafe fn cursor_rect_cursor(image: CursorImageId) -> id {
    let cursor = CURSORS
        .with_borrow(|cursors| cursors.shown.get(&image).map(|(_, cursor)| cursor.clone()))
        .unwrap_or_else(NSCursor::arrowCursor);
    Retained::autorelease_ptr(cursor).cast()
}

/// The cursor `id` shows, if it points at a picture.
pub(crate) fn shown(image: CursorImageId) -> Option<Retained<NSCursor>> {
    CURSORS.with_borrow(|cursors| cursors.shown.get(&image).map(|(_, cursor)| cursor.clone()))
}

/// Invalidates the cursor rects of every GPUI window whose view shows
/// `image`'s cursor, and sets the cursor now when the pointer is over the view
/// of the window that takes the pointer's cursor (the key window, else the
/// main one, as `set_cursor_style` picks) and the application is active.
///
/// A window that is not key keeps the rects it registered, with the cursor
/// they held then, until they are invalidated; invalidating them here means
/// the window shows the id's latest picture when it is key again, rather than
/// the one it showed when it last was.
///
/// # Safety
///
/// Must run on the AppKit main thread.
unsafe fn show_now(image: CursorImageId) {
    // SAFETY: the caller guarantees the main thread. `is_gpui_window` checks
    // each window carries GPUI's state ivar before `get_window_state` reads it;
    // `windows` is an array the application keeps alive for the call.
    unsafe {
        let app = NSApplication::sharedApplication(nil);
        let key_window: id = msg_send![app, keyWindow];
        let main_window: id = msg_send![app, mainWindow];
        let cursor_window = [key_window, main_window]
            .into_iter()
            .find(|window| !window.is_null() && is_gpui_window(*window));
        let windows: id = msg_send![app, windows];
        let count: NSUInteger = msg_send![windows, count];
        for index in 0..count {
            let window: id = msg_send![windows, objectAtIndex: index];
            if !is_gpui_window(window) {
                continue;
            }
            let state = get_window_state(&*window);
            let (native_window, native_view, style) = {
                let state = state.lock();
                (
                    state.native_window,
                    state.native_view.as_ptr() as id,
                    state.cursor_style,
                )
            };
            if style != CursorStyle::Image(image) {
                continue;
            }
            let _: () = msg_send![native_window, invalidateCursorRectsForView: native_view];
            let active: bool = msg_send![app, isActive];
            if cursor_window == Some(window)
                && active
                && pointer_over(native_window, native_view)
                && let Some(cursor) = shown(image)
            {
                cursor.set();
            }
        }
    }
}

/// Whether the pointer is over `view` and no other window is in front of it
/// there.
///
/// # Safety
///
/// Must run on the AppKit main thread with `window` a live `NSWindow` and
/// `view` a live view in it.
unsafe fn pointer_over(window: id, view: id) -> bool {
    // SAFETY: the caller guarantees the main thread and live objects; every
    // message here is a plain NSEvent, NSWindow or NSView query.
    unsafe {
        let on_screen: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let in_front: NSInteger = msg_send![
            class!(NSWindow),
            windowNumberAtPoint: on_screen
            belowWindowWithWindowNumber: 0 as NSInteger
        ];
        let number: NSInteger = msg_send![window, windowNumber];
        if in_front != number {
            return false;
        }
        let in_window: NSPoint = msg_send![window, convertPointFromScreen: on_screen];
        let in_view: NSPoint = msg_send![view, convertPoint: in_window fromView: nil];
        let bounds: NSRect = msg_send![view, bounds];
        in_view.x >= bounds.origin.x
            && in_view.y >= bounds.origin.y
            && in_view.x < bounds.origin.x + bounds.size.width
            && in_view.y < bounds.origin.y + bounds.size.height
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::{CursorImage, CursorImageId, DevicePixels, point, size};
    use objc2_app_kit::NSCursor;
    use objc2_core_graphics::{CGDataProvider, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};

    use super::{BUILT_KEPT, Cursors, build};

    /// A `side`-pixel square whose every pixel is `bgra`, with the hotspot at
    /// (`hot`, `hot + 1`).
    fn square(side: i32, bgra: [u8; 4], hot: i32, scale: f32) -> CursorImage {
        let pixels: Vec<u8> = bgra.repeat((side * side) as usize);
        CursorImage::new(
            pixels,
            size(DevicePixels(side), DevicePixels(side)),
            point(DevicePixels(hot), DevicePixels(hot + 1)),
            scale,
        )
        .unwrap()
    }

    /// The pixels of the cursor's one representation, read back at its own
    /// pixel size: premultiplied BGRA, as the picture was given.
    fn pixels_of(cursor: &NSCursor) -> (usize, usize, Vec<u8>) {
        let reps = cursor.image().representations();
        assert_eq!(reps.len(), 1, "one representation, the picture's");
        let rep = reps.objectAtIndex(0);
        // SAFETY: a null proposed rect asks for the whole representation at its
        // own pixel size; no context and no hints is the documented default.
        let picture =
            unsafe { rep.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
                .expect("a CGImage");
        let some = Some(&*picture);
        let (wide, high) = (CGImage::width(some), CGImage::height(some));
        assert_eq!(CGImage::bits_per_pixel(some), 32);
        assert_eq!(CGImage::bytes_per_row(some), wide * 4, "rows packed");
        assert_eq!(
            CGImage::alpha_info(some),
            CGImageAlphaInfo::PremultipliedFirst
        );
        assert_eq!(
            CGImage::byte_order_info(some),
            CGImageByteOrderInfo::Order32Little
        );
        let provider = CGImage::data_provider(some).expect("pixels");
        let data = CGDataProvider::data(Some(&provider)).expect("bytes");
        // SAFETY: the data is a copy CoreGraphics made for this call, which
        // nothing else holds or changes.
        (wide, high, unsafe { data.as_bytes_unchecked() }.to_vec())
    }

    #[test]
    fn a_cursor_is_its_picture_in_points_with_the_hotspot_in_points() {
        let image = square(64, [0, 0, 255, 255], 11, 2.);
        let cursor = build(&image).expect("a cursor");
        let points = cursor.image().size();
        assert_eq!((points.width, points.height), (32., 32.), "64 pixels at 2×");
        let hot = cursor.hotSpot();
        assert_eq!(
            (hot.x, hot.y),
            (5.5, 6.),
            "pixel (11, 12) at 2×, from the top left"
        );
        let (wide, high, pixels) = pixels_of(&cursor);
        assert_eq!((wide, high), (64, 64), "the picture's own pixels");
        assert_eq!(pixels, image.bgra(), "byte for byte, BGRA as given");
    }

    /// The cursor's picture drawn by CoreGraphics into an sRGB bitmap of RGBA
    /// bytes: a byte-for-byte check of the pixels would pass with the channels
    /// swapped, so this checks what the bitmap info makes of them. Red, green,
    /// blue and half-covered white, given as premultiplied BGRA.
    #[test]
    fn a_cursor_draws_its_pictures_colours_not_its_byte_order() {
        use objc2_core_foundation::{CGPoint, CGRect, CGSize};
        use objc2_core_graphics::{
            CGBitmapContextCreate, CGColorSpace, CGContext, kCGColorSpaceSRGB,
        };

        let bgra = [
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [128, 128, 128, 128],
        ];
        let image = CursorImage::new(
            bgra.concat(),
            size(DevicePixels(2), DevicePixels(2)),
            point(DevicePixels(0), DevicePixels(0)),
            1.,
        )
        .unwrap();
        let cursor = build(&image).expect("a cursor");
        let rep = cursor.image().representations().objectAtIndex(0);
        // SAFETY: as in `pixels_of`.
        let picture =
            unsafe { rep.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
                .expect("a CGImage");
        let mut rgba = [0u8; 16];
        // SAFETY: `kCGColorSpaceSRGB` is an immutable CFString constant
        // CoreGraphics exports for the process's lifetime.
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
        // SAFETY: `rgba` holds 2 × 2 pixels of 4 bytes, rows of 8 bytes, and
        // outlives the context, which is dropped at the end of the test.
        let context = unsafe {
            CGBitmapContextCreate(
                rgba.as_mut_ptr().cast(),
                2,
                2,
                8,
                8,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }
        .expect("a bitmap context");
        let whole = CGRect::new(CGPoint::new(0., 0.), CGSize::new(2., 2.));
        CGContext::draw_image(Some(&context), whole, Some(&picture));
        drop(context);
        assert_eq!(
            rgba.as_chunks::<4>().0,
            [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [128, 128, 128, 128]
            ],
            "red, green, blue, half white: the bytes read as BGRA, alpha premultiplied"
        );
    }

    #[test]
    fn a_translucent_picture_keeps_its_premultiplied_colour() {
        let image = square(2, [0, 64, 0, 128], 0, 1.);
        let (_, _, pixels) = pixels_of(&build(&image).expect("a cursor"));
        assert_eq!(
            pixels,
            image.bgra(),
            "not multiplied by alpha a second time"
        );
    }

    #[test]
    fn an_id_pointed_back_at_a_picture_it_showed_builds_nothing() {
        let mut cursors = Cursors::default();
        let (arrow, beam) = (square(4, [0, 0, 0, 255], 0, 2.), square(4, [255; 4], 1, 2.));
        let tile = CursorImageId(7);
        assert!(cursors.set(tile, Some(&arrow)));
        let first = cursors.shown[&tile].1.clone();
        assert!(
            !cursors.set(tile, Some(&arrow)),
            "the same picture changes nothing"
        );
        assert!(cursors.set(tile, Some(&beam)));
        assert!(cursors.set(tile, Some(&arrow.clone())));
        assert_eq!(cursors.builds, 2, "the arrow was built once");
        assert!(
            std::ptr::eq(&*cursors.shown[&tile].1, &*first),
            "and the same NSCursor came back"
        );
        assert!(cursors.set(tile, None), "forgotten");
        assert!(!cursors.set(tile, None), "twice changes nothing");
    }

    #[test]
    fn the_cursors_kept_are_bounded_and_the_used_ones_stay() {
        let mut cursors = Cursors::default();
        let first = square(2, [1, 2, 3, 255], 0, 1.);
        cursors.set(CursorImageId(0), Some(&first));
        for n in 1..=(BUILT_KEPT as u8 + 8) {
            // Show the first picture again now and then so it is recently used.
            if n % 16 == 0 {
                cursors.set(CursorImageId(1), Some(&first));
            }
            cursors.set(CursorImageId(1), Some(&square(2, [n, 0, 0, 255], 0, 1.)));
        }
        assert_eq!(cursors.built.len(), BUILT_KEPT);
        let builds = cursors.builds;
        cursors.set(CursorImageId(2), Some(&first));
        assert_eq!(cursors.builds, builds, "a picture in use was kept");
    }

    /// What a shape change costs on the main thread, and that a cursor set is
    /// AppKit's current cursor as soon as `set` returns: no frame, no run-loop
    /// turn. Prints `MEASURE` lines; the bounds are loose ceilings, not the
    /// numbers.
    #[test]
    fn a_shape_change_is_built_once_and_set_in_microseconds() {
        fn percentile(mut samples: Vec<Duration>, p: usize) -> Duration {
            samples.sort();
            samples[(samples.len() - 1) * p / 100]
        }
        let shapes: Vec<CursorImage> = (0..32u8)
            .map(|n| square(64, [n, 255 - n, 0, 255], 8, 2.))
            .collect();
        let mut cursors = Cursors::default();
        let tile = CursorImageId(1);
        let mut cold = Vec::new();
        for shape in &shapes {
            let started = Instant::now();
            cursors.set(tile, Some(shape));
            cold.push(started.elapsed());
        }
        let mut warm = Vec::new();
        let mut set = Vec::new();
        for round in 0..20 {
            for shape in &shapes {
                let started = Instant::now();
                cursors.set(tile, Some(shape));
                warm.push(started.elapsed());
                let cursor = cursors.shown[&tile].1.clone();
                let started = Instant::now();
                cursor.set();
                let current = NSCursor::currentCursor();
                set.push(started.elapsed());
                if round == 0 {
                    assert!(std::ptr::eq(&*current, &*cursor), "set is current at once");
                }
            }
        }
        for (name, samples) in [("build", cold), ("cached", warm), ("set", set)] {
            let (p50, p99) = (percentile(samples.clone(), 50), percentile(samples, 99));
            eprintln!("MEASURE cursor_image_{name} p50={p50:?} p99={p99:?}");
            assert!(p50 < Duration::from_millis(5), "{name} p50 {p50:?}");
        }
    }
}
