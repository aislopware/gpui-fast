# Window composition

How a GPUI window shows native views and layers (a web view, a video layer) inside its
own content: ordered, clipped, rounded and faded with what GPUI paints around them, taking
the pointer and the keyboard where GPUI's own hit test and focus say, and moving with the
GPUI content around them on the same refresh. What was measured, with the commands, and the
decisions the numbers settled are below.

The code: `crates/gpui/src/fast/composition/` (the model, platform-neutral),
`crates/gpui_apple/src/metal_renderer.rs` (holes and the blend),
`crates/gpui_apple/src/fast/video_layer.rs` (`VideoLayer`),
`crates/gpui_macos/src/fast/composition.rs` and `crates/gpui_ios/src/ios/composition.rs`
(the platforms). Hooks in upstream files are one-liners and are allowlisted in
`script/upstream-allowlist`.

## The model

**A native's placement is part of the frame.** `native_view(&host)` (or
`Window::paint_native` from a custom element) paints a `NativePlacement` into the scene at
its place in painter's order: its bounds, the content mask it is clipped by, its corner
radii and the element opacity. The placement is a scene operation, so a view that retained
mode draws from the last frame replays it with its primitives (`PaintOperation::Native`).
A native that no element placed in a frame is hidden.

**Natives are under GPUI.** The platform keeps each native in a container view below
GPUI's single Metal layer. Every placement also paints a *hole*: a quad drawn with blend
factors `src × 0 + dst × (1 − coverage × opacity)` for colour and alpha, which clears GPUI's
drawable where the native shows (antialiased, rounded, faded by the opacity). Core Animation
composites premultiplied, `out = G + (1 − α_G) × N`, so the native shows through the hole
exactly as if GPUI had drawn it there, and everything painted after the placement lands on
top of the hole and so on top of the native: popovers, menus, toasts, the drag preview,
focus rings, a remote pointer. There is one GPUI layer and no overlay layer per native.

- **The blend fix.** GPUI's pipelines blended destination alpha with `One`, which is right
  only for an opaque drawable. The non-path pipelines and the path sprite pipeline now use
  `OneMinusSourceAlpha` for destination alpha ("over"), so alpha stays coverage where GPUI
  paints over a hole. For an opaque drawable the result is bit-identical (tested).
- **Ties.** A hole is ordered as the native's own quad would be: after shadows of the same
  order (shadows draw beyond their bounds-tree bounds, about three blur radii), before any
  other primitive of the same order.
- **Paint layers.** A native painted inside a paint layer re-inserts the layer's bounds
  into the bounds tree, so the rest of the layer gets an order above the native, as it
  would above a quad.
- **The GPUI layer** is marked non-opaque while any hole shows and opaque again when none
  does, and hidden for a frame where one opaque native covers the whole window with nothing
  of GPUI's above it (`NativeFrame::covers_window`).

**Presenting.** Per presented frame, the window builds a `NativeFrame` (placements in
device pixels, visible rect, opacity and stacking rank, deduplicated) and a
`NativeHitMap`, and hands both to `PlatformWindow::present_natives`. The platform compares
the frame with the last one it presented (`NativeFrame::changes_since`). A frame that
changes nothing native is presented as before. A frame that places, moves, restacks or
hides a native, or flips the GPUI layer's opacity, is *transactional*: inside one explicit
`CATransaction` with actions disabled, the Metal layer presents with the transaction
(`presentsWithTransaction`, commit, `waitUntilScheduled`, `present`), the containers get
their frames, `zPosition`s and visibility, and the transaction commits, so the hole and the
native reach the glass on the same refresh. Containers are added once and never removed or
re-added while their host lives (a web view that leaves its window reloads its process),
and they are stacked by `zPosition` below GPUI's layer, never by reordering subviews.

**Pointer.** The hit map lists, topmost first, each native's visible rect and the rects of
hitboxes inserted after its own (the occluders): `native_at(p)` is the native whose hitbox
is the first hitbox GPUI's own hit test finds at `p` (tested against `Frame::hit_test` on
random scenes). The GPUI view answers `hitTest:` with nil where a native is topmost, and a
container answers only where its native is, so the platform delivers the event there. GPUI
still sees a press that lands on a native, as the native element's hitbox is topmost: on
macOS the window's `sendEvent:` shows it to GPUI first (menus close on `mouse_down_out`,
the element's `on_mouse_down` runs) and then lets it go to the native, with the drag and
release that follow; on iOS a gesture recogniser on the root view that never recognises,
cancels or delays shows GPUI a touch that begins over a native as a press. Scrolling over
a native goes to the native only. The cursor is left to the native where its hitbox is
topmost.

**Keyboard.** `native_view(..).track_focus(&handle)` ties the native to a focus handle.
GPUI to platform: after each present, if GPUI's focus is on the native's element, the
native's view is made first responder; if focus left it while the native held the keyboard,
the GPUI view takes it back. Platform to GPUI: when the user moves the first responder into
a native (`makeFirstResponder:` on macOS; on iOS the first responder is looked up after
each touch sequence and before each present), GPUI focuses the element; out of it, GPUI
blurs it. On macOS a key press while a native holds the keyboard is offered to GPUI's
keymap first (the `NativeView` key context applies) and consumed only if a binding matches;
the GPUI view's `performKeyEquivalent:` leaves unmatched equivalents (⌘C, ⌘V, ⌘Z) to the
native's responder chain. Nothing is offered while the native composes text with an input
method.

**Video** does not go through the GPUI frame at all: see `VideoLayer` below.

## API

```rust
use gpui::composition::{NativeHost, NativeHostOptions, native_view};

// Once, when the content is created (main thread). Fails where the platform cannot
// compose (Linux, Windows, the web).
let host: NativeHost = window.create_native_host(
    NativeHostOptions { opaque: true, interactive: true, label: Some("Web page".into()) },
    cx,
)?;
// SAFETY: a live NSView* (macOS) / UIView* (iOS), on the main thread.
unsafe { host.attach_view(NonNull::new(view_ptr).unwrap())? };
// or a CALayer*: unsafe { host.attach_layer(layer_ptr)? };

// Every frame it should show, in render: a styled, interactive element.
native_view(&host)
    .size_full()
    .rounded(px(8.))
    .track_focus(&self.focus)
    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| { /* focus the tile */ }))
```

The host is `Clone`; the native leaves the window when the last clone is dropped. Children
of `native_view` are GPUI content drawn over the native. `Window::native_hit_map()` and
`Window::presented_natives()` expose what the last present did, for tests; the test
platform records hosts (`TestNativeHost`: placement, hidden, platform calls,
`simulate_focus`).

```rust
use gpui_apple::fast::video_layer::{VideoLayer, VideoLayerOptions, VideoPresentedSink};

let host = window.create_native_host(NativeHostOptions::default(), cx)?;
let video = VideoLayer::attach(&host, VideoLayerOptions::default())?; // main thread
video.on_presented(Some(Arc::new(|sequence, frame: PresentedFrame| { /* decoded→glass */ })));
// On the decoding thread, for each CVPixelBuffer (any format the surface element takes):
let sequence = video.present(&pixel_buffer)?; // never blocks on the display
// In render: place the host where the picture goes; paint pointer, HUD and readout after.
native_view(&host).absolute().left(fit.origin.x).top(fit.origin.y).size(fit.size)
```

`VideoLayer` is `Clone + Send + Sync`. The picture is drawn by the surface shader
(Y'CbCr matrix and range from the buffer, 8- and 10-bit, 4:2:0 and 4:4:4) into a drawable
the size of the picture, stretched to the layer, on the layer's own thread. At most one drawn
picture is on its way to the glass; a picture handed over meanwhile waits in a one-picture
mailbox, is drawn the moment the previous one is shown, and is replaced by a newer one
(reported with no `presented_at`).

## How Slopty adopts it

Slopty pins this branch and depends on `gpui_apple` directly on Apple targets for
`VideoLayer`.

- **Browser tile** (`slopty-platform::web`, `slopty-ui::browser`): the tile creates one
  `NativeHost` per page with its label, attaches the `WKWebView` with `attach_view`, and
  renders `native_view(&host).size_full().track_focus(&page_focus)` as the body. `place`,
  `hide`, `Cover`, `placement`, the `MIN_ALPHA` hiding, `browser_sync`, the toast clip and
  the snapshot-while-covered all go: menus, the palette and toasts over a page are drawn over
  its hole, and the page stays shown. The Mac key monitor and `web::edit_for` go too, since
  keys reach GPUI's keymap first and unmatched equivalents reach WebKit. The snapshot stays
  for the overview (scaled tiles) and for goldens (`render_to_image` does not capture
  natives). Moving a page to another window means a new host in that window and
  `attach_view` again.
- **Stream** (`ScreenView`): one `VideoLayer` per stream, attached to a host the view
  keeps. The decoder calls `video.present(&buffer)` on its own thread instead of updating
  the entity; `render` places `native_view(&host)` at the fitted picture rect and paints
  only the pointer, readout and HUD after it. Pointer samples then draw on their own. A
  popped-out stream window gets its own host and `VideoLayer`. Zoom is a smaller or larger
  placement clipped by the tile (the content mask clips the container).
- **Measuring**: `slopty-glass` / `tests/app/stream.rs` take decoded→glass from
  `on_presented` and the window's frame count from `Window::on_frame_presented`.

## Checks

| What | Command |
| --- | --- |
| Model, retained replay, focus, hit map against GPUI's hit test, covers-window | `cargo test -p gpui --lib fast::tests::composition` |
| Retained-mode oracle with natives in panels, clipped and faded containers and list rows | `cargo test -p gpui --lib fast::tests::oracle` (and with `GPUI_VIEW_RETENTION=0`) |
| Holes composited with natives under them against one pass drawing the natives | `cargo test -p gpui_apple --lib composition_tests` |
| macOS window: containers, hit testing, transactional frames, pointer and focus both ways, video without GPUI frames | `cargo test -p gpui_perf --test composition_macos` |
| Same refresh, live (and the negative control) | `COMPOSITION_LIVE=1 GPUI_PRESENTED_AT_CALLBACK=1 cargo test -p gpui_perf --release --test composition_macos`, then with `GPUI_COMPOSITION_TRANSACTIONS=0` |
| iOS simulator: root view, safe area, containers, hit testing, transactional frames, focus both ways | see below |

The pixel oracle renders random scenes (quads, shadows, underlines, paths, paint layers
and non-overlapping natives of sentinel colours) twice: GPUI with holes composited over the
natives as Core Animation does, and one pass drawing each native as a quad. In
`RGBA16Float` the worst channel differs by 0.623 of an 8-bit step and no pixel by more than
one step. In the drawable's `BGRA8Unorm` rounding happens twice (once in GPUI's drawable,
once in the composite), so the worst channel is 3 steps off, on 642 of 38.4 million
channels (under 1 in 10⁴); the test asserts both bounds.

The iOS checks run inside the simulator app:

```text
crates/gpui_ios/examples/ios/build-simulator.sh
xcrun simctl boot 'iPhone 18 Pro'
xcrun simctl install booted target/gpui-ios-example-xcode/Build/Products/Debug-iphonesimulator/GPUIIosExample.app
SIMCTL_CHILD_GPUI_IOS_COMPOSITION_TEST=1 xcrun simctl launch --console-pty booted dev.zed.gpui-ios-example
xcrun simctl shutdown booted
```

All 19 checks pass on iOS 27.0 (iPhone 18 Pro simulator), the safe area GPUI reports
among them (62 and 34 pt, the window's own). Touches cannot be synthesised in-process
there, so the recogniser's press is not exercised by a test; its configuration is.

The live tests open one non-activating panel above other windows and never take the
keyboard; their events are made in-process and sent to their own window.

## Measurements

Mac Studio (M1 Max), macOS 27.0.1, a 1920×1080 60 Hz virtual display driven by Parsec.
That display never reports scan-out (`presentedTime` is 0 on every drawable), so every
"to glass" number here takes the drawable's presented handler as the glass
(`GPUI_PRESENTED_AT_CALLBACK=1`); the handler runs once the window server has composited
the drawable. Parsec's encoder and another app's GPU work were running throughout, so the
window server's GPU time moves by ±0.1 ms per refresh between identical runs; compare
within a round.

`composition_latency` modes: `idle` is one frame after a quarter second of idle, from
another thread's notify (a keystroke in a quiet terminal), `wake_to_glass` from the notify;
`continuous` is a frame every refresh, `main_cpu_per_frame` the main thread's CPU time per
frame. Window server GPU time is the union of its Metal intervals in a 10 s Instruments
trace of the continuous run.

```text
cargo build -p gpui_perf --release --example composition_latency --example windowserver_gpu
GPUI_PRESENTED_AT_CALLBACK=1 target/release/examples/composition_latency <variant> idle
GPUI_PRESENTED_AT_CALLBACK=1 COMPOSITION_FRAMES=1200 target/release/examples/composition_latency <variant> continuous
xcrun xctrace record --template 'Metal System Trace' --all-processes --time-limit 10s --output /tmp/ws.trace   # during the continuous run
target/release/examples/windowserver_gpu /tmp/ws.trace --refresh-hz 60
```

### Stage 0: the underlay against an overlay band

Variants: `today` (GPUI's layer opaque, a native hidden under it), `nonopaque` (GPUI's
layer non-opaque over the native: the underlay's cost with no hole), `band` (a second
full-window transparent `CAMetalLayer` above GPUI's, presented every frame: zed#62379's
overlay).

| Round | Variant | wake→glass p50 / p95 ms | main CPU/frame p50 ms | submit→glass p50 ms | WindowServer GPU ms/refresh |
| --- | --- | --- | --- | --- | --- |
| 1 | today | 15.15 / 22.90 | 0.22 | 12.59 | 0.391 |
| 1 | nonopaque | 14.68 / 20.99 | 0.23 | 12.66 | 0.395 |
| 1 | band | 13.75 / 21.40 | 0.30 | 11.42 | 0.416 |
| 2 | today | 17.28 / 21.57 | 0.24 | 11.44 | 0.428 |
| 2 | nonopaque | 15.03 / 22.12 | 0.27 | 11.37 | 0.431 |
| 2 | band | 15.19 / 22.57 | 0.37 | 12.60 | 0.327 |

Making GPUI's layer non-opaque costs the window server +0.004 and +0.003 ms of GPU per
refresh and nothing measurable to glass, well inside the fallback threshold (the band
would have to be ≥ 0.2 ms/refresh and ≥ 0.5 ms p50 cheaper). The band costs the main
thread +0.08 / +0.13 ms per frame for its extra drawable. **Decision: the underlay stays;
no overlay band.**

### Stage 3: holes and transactional frames on macOS

Variants: `holes` (a native placed with `native_view`, GPUI cutting its rounded hole) and
`moving` (the same native moving a point every frame, so every frame is transactional).

| Round | Variant | wake→glass p50 / p95 ms | main CPU/frame p50 ms | submit→glass p50 ms |
| --- | --- | --- | --- | --- |
| 1 | today | 17.08 / 24.04 | 0.22 | 11.69 |
| 1 | holes | 17.05 / 24.67 | 0.29 | 12.00 |
| 1 | moving | 19.55 / 25.87 | 0.43 | 11.86 |
| 2 | today | 16.23 / 22.83 | 0.23 | 13.34 |
| 2 | holes | 16.28 / 25.62 | 0.22 | 14.18 |
| 2 | moving | 15.96 / 23.85 | 0.32 | 13.96 |

Acceptance, all met:

1. A visible, still native adds −0.03 / +0.05 ms p50 wake→glass to today (limit +0.5).
2. Transactional frames add +2.50 / −0.32 ms p50 wake→glass to plain ones with holes
   (limit +4.5).
3. The main thread spends +0.14 / +0.10 ms more per transactional frame (limit +0.3):
   the `waitUntilScheduled` and the container updates.
4. Same-refresh test: 0 seams in 625 and in 627 frames (two runs) with transactions,
   the two drawables' presentations 0.02 ms apart at the median. With
   `GPUI_COMPOSITION_TRANSACTIONS=0` the same runs had 625 of 625 and 11 of 627 frames
   where the native and its hole reached the glass on different refreshes: how often
   depends on where in the refresh the frame lands, but the test sees the seam it guards
   against.

The window server spent 0.637 ms/refresh with `today` and 0.608 with `holes` in the same
round (round 1; the other GPU load was higher than in stage 0). Recording the `moving` run
was abandoned: `xctrace` spent minutes post-processing that trace.

### Stage 6: VideoLayer against the surface element

`video_latency` hands a 1920×1080 4:2:0 picture every 1/60 s from a producer thread,
either to a GPUI view that shows it with `surface` (a GPUI frame per picture) or to a
`VideoLayer`, or both side by side (`both`), which compares the same pictures and so
removes the drift between the producer's clock and the display's (60 against 59.77 Hz
here) that makes separate runs differ by several milliseconds. `VIDEO_BUILD_MS` makes
every GPUI frame spend that long on the main thread first, as a window with more in it
does.

```text
cargo build -p gpui_perf --release --example video_latency
GPUI_PRESENTED_AT_CALLBACK=1 target/release/examples/video_latency <surface|layer|both>
GPUI_PRESENTED_AT_CALLBACK=1 VIDEO_BUILD_MS=12 target/release/examples/video_latency both
```

| Run | decoded→glass p50 / p95 ms | dropped / 600 | GPUI frames/s | main CPU per picture ms |
| --- | --- | --- | --- | --- |
| surface alone (2 runs) | 17.70 / 21.99, 11.70 / 25.39 | 1, 10 | 56.5, 55.7 | 0.221, 0.220 |
| layer alone (2 runs) | 21.88 / 24.89, 19.52 / 22.68 | 0, 9 | **0.0**, **0.0** | **0.034**, **0.040** |
| both, paired: layer − surface p50 (2 runs) | +0.02, +0.02 | | | |
| both, 5 ms build: layer − surface p50 (2 runs) | −0.02, −0.02 | | | |
| both, 12 ms build: layer − surface p50 (2 runs) | **−16.72**, +0.01 | | | |

- When GPUI's frame is light, a picture reaches the glass on the same refresh either way;
  the video layer's gain is everything else: the window draws no frame for video (0 against
  56 per second), and the main thread spends 0.03–0.04 ms per picture instead of the whole
  frame (0.22 ms here with one element; Slopty's window build is several milliseconds).
- When GPUI's frame is heavy enough to miss the refresh, the surface path shows the picture
  a refresh later (−16.7 ms p50 for the layer in the run where the 12 ms build crossed the
  deadline) and the layer is unaffected; whether a given build crosses depends on the phase
  of the pictures against the refresh, hence the second run.
- **The mailbox.** The first version presented every picture as it came, with three
  drawables: a producer a little faster than the display filled the drawable queue, and
  decoded→glass settled at 36.5 / 42.0 ms p50 (37.3 with two drawables), against 13.8 /
  20.1 ms for the surface path, which GPUI's pacer keeps from queueing. With at most one
  picture on its way to the glass and the newest waiting in a one-picture mailbox it is
  19.5–21.9 ms alone and equal to the surface path when paired.

The design's Stage 6 acceptance (decoded→glass p50 ≥ 4 ms lower than Slopty's
decoded→painted plus paint→glass on loopback) is measured on Slopty's side, where the
window build is the cost the layer removes.

## Decisions

- **Underlay with holes, not overlay bands.** Stage 0: the non-opaque GPUI layer costs the
  window server ≈0.004 ms per refresh; a band costs a drawable per frame and cannot put GPUI
  content between two natives.
- **Transactions only on frames that change natives.** A plain frame keeps GPUI's
  asynchronous present and its pacing; the transactional one costs +0.1 ms of main thread.
- **`zPosition`, not subview order.** Restacking by moving views would remove them from the
  window for a moment, which reloads a web view's process and drops its first responder.
- **Hit testing from the presented frame.** The platform asks the hit map of the frame on
  glass, so `hitTest:` never re-enters the app and agrees with what the user sees.
- **Video presents off the main thread through a mailbox.** Queueing presents added two
  refreshes; the mailbox keeps one picture in flight and drops the stale one.
- **`VideoLayer` draws with GPUI's renderer** (a `MetalRenderer` of its own drawing one
  surface primitive), so its colour handling is the surface element's by construction.

## Limits and what is left

- **Translucent natives** (`NativeHostOptions::opaque = false`) are drawn as if opaque:
  what GPUI painted under a native is cleared by its hole. A native faded by element
  opacity blends with what is *under the GPUI layer* (the window background), not with
  GPUI's content under it. Stage 7 of the design (bounding-box bands) is not built.
- **Overlapping natives** each clear their own hole; a native over another native works
  (stacking by rank), but a rounded or faded native over another shows the lower native, not
  GPUI content, in its corners.
- **8-bit rounding**: in the drawable's format a hole's edge can differ from one-pass
  drawing by up to 3 steps on under 1 in 10⁴ channels.
- **Multi-stroke key bindings** while a native holds the keyboard do not replay a pending
  first stroke to the native. **iOS hardware keys** are not offered to GPUI's keymap while a
  native holds the keyboard (`UIKeyCommand`s for bound chords are not registered).
- **Accessibility**: the containers are real subviews, so VoiceOver reaches the native, but
  GPUI's accesskit tree has no placeholder node at the element yet.
- **VideoLayer**: 10-bit output (`RGBA16Float` / `bgr10a2` drawables), the IOSurface
  `contents` variant (6b) and the full-screen direct-to-display check (6c) are not done; the
  drawable is `BGRA8Unorm`. A stress run of presents while the tile animates, and a ΔE colour
  test against the surface element, are left too (the colour path is the same code).
- **Presentation timing** on this machine is the presented-handler proxy (see above); a
  display that reports `presentedTime` gives the scan-out time instead, with no change to
  the tests.
