# Slopty's fork of gpui-fast

`aislopware/gpui-fast` is [longbridge/gpui-fast](https://github.com/longbridge/gpui-fast)
(GPUI with Retained Mode) plus what Slopty needs from GPUI: the iOS backend, video
surfaces, presentation reports and pacing, and a few smaller features. It replaces the
zed fork (`aislopware/zed`) Slopty used before. Everything lives on `main`; there is no
separate branch for our commits.

Two upstreams feed it:

- **longbridge/gpui-fast** (`origin/main`), merged in as it moves.
- **zed-industries/zed**, imported through gpui-fast's own vendor procedure
  ([docs/upstream-sync.md](docs/upstream-sync.md)), so this fork follows zed even when
  longbridge has not taken a newer zed yet.

## History layout

- `zed-vendor`: the vendor branch. Every commit on it is `zed: import <short hash>` and
  holds nothing but zed's copy of the directories `UPSTREAM` lists. It starts at
  gpui-fast's first import (`11a44c4`, zed `7960b2a7`). The latest vendor commit is
  `2db56fa` (`zed: import bd747337`, zed `bd747337d7be`).
- `main`: gpui-fast's history, a merge of each vendor commit (`acfc6db`, "Merge zed
  bd747337 into gpui-fast"), our commits, and merges of longbridge's `main`. The last
  longbridge commit merged is `ac1c226` (#12, "hand a text element's inputs to its
  measurement instead of copying them"), in `751acaf`.
- `UPSTREAM` records `zed_commit` and `import_commit` (the latest vendor commit).
  `script/check-upstream` compares every tracked file against it.

## What the fork carries on top of gpui-fast

Ported from `aislopware/zed` (`origin/main..main` on zed `bd747337`), in order, with their
authors and messages:

- `d6093a1` gpui_ios: iOS platform backend from #63068, on top of 801c087
- `d253e07` gpui: surface element and CoreVideo texture path on iOS
- `5310332` gpui: expose Window::insets
- `526215e` gpui_ios: pinch to zoom via UIPinchGestureRecognizer
- `99dae06` gpui_ios: hardware keyboard presses, key repeat, trackpad hover and scroll
- `ff17943` gpui_ios: render_to_image through an offscreen Metal texture
- `5d79996` gpui: expose the accessibility tree to tests; gpui_ios: VoiceOver bridge
- `bca8639` gpui: expose the full FontMetrics and a ShapedLine's layout
- `30f96ea` gpui: paint a glyph rasterized at one size and drawn at another
- `6ac4bec` gpui_ios: deliver described input at the UIKit boundary for tests
- `16e31ec` gpui_ios: take the input handler and callback out of their cells for the call
- `f1e9a18` gpui_ios: caps lock reverses shift on letters in the US stand-in
- `c050c8c` gpui_ios: prevent idle sleep through the UIKit idle timer
- `bdaa979` gpui_ios: pictures on the pasteboard, both ways
- `a3aaefa` gpui: shape a line's font runs by font, not by colour
- `67fb314` gpui: add CursorStyle::None to hide the pointer over an element
- `a41085b` gpui_ios: window visibility from the app lifecycle, no system sleep
- `a497ee9` gpui: the secondary modifier is ⌘ on iOS as on macOS
- `5edb11a` gpui_macos: a cancelled scroll or pinch is cancelled, not moved
- `03e7ae6` gpui: pass the glyph atlas key by value after #64331
- `a33830d` gpui_apple: sample video surfaces by their own Y'CbCr matrix, and never abort on one
- `38cbd2b` feat(gpui): report when each frame reaches the display
- `75679a7` feat(gpui_ios)!: own the display link and pause it while idle
- `4f7a1e1` test(gpui): measure submit-to-glass with a frame_latency example
- `7f3613f` perf(gpui_apple): stop frames queueing in front of the display
- `8758f95` fix(gpui): treat a file drag as mouse input
- `ae1ef6b` fix(gpui_apple): skip presentation reports where drawables lack them
- `7287bf5` fix(gpui_macos): keep the immediate frame for a wave that drew nothing
- `2eae873` test(gpui): time a keystroke's frame beside a window that draws every refresh
- `6c13772` fix(gpui): focus the nearest ancestor node when the focused element has none
- `c5f0200` feat(gpui): an outline style, a ring drawn clear of the element
- `c10d8a9` gpui_macos: keep drawing a covered window in test builds
- `ea73091` feat(gpui_apple): draw 10-bit and 4:4:4 video surfaces

Added in this fork:

- `1776aa2` test(gpui): a video surface shows the buffer its view holds, retained or not
- `4d0009e` gpui: build a view that asked for an animation frame on the next frame drawn

### Candidates for longbridge

Generic to gpui-fast, not to Slopty, and worth a pull request to longbridge/gpui-fast:

- the zed bd747337 sync itself: `2db56fa` (`zed: import bd747337`) and its merge `acfc6db`,
  including the font generation handling (`Window::refresh_if_fonts_changed`, a text
  measurement carried over only within one font generation, and its test
  `every_view_is_rendered_again_once_fonts_are_added`);
- `4d0009e`, a view that asked for an animation frame built on the next frame drawn;
- `1776aa2`, the retained-mode test of a surface's buffer (macOS; the surface element is
  upstream's).
- the commit adding this file, which also lets `script/check-upstream` accept the patches
  above (`script/upstream-allowlist`, the "Slopty's patches" section)

`crates/gpui_ios` is ours, like `crates/gpui_perf` is gpui-fast's: it is a workspace member
but not in `UPSTREAM`'s `tracked` list, so `script/check-upstream` does not look at it.

Our patches to upstream files stay in those files, as they were in the zed fork, rather
than being moved into `fast/`: they are features, not retained-mode hooks. The allowlist
gives each such file budgets just above what it adds now, so growth still shows up.

## Retained Mode and our patches

Retained Mode draws a view from the last frame while nothing it read changed
([docs/retained-mode.md](docs/retained-mode.md)). For what we added:

- **Video surfaces.** A `surface(buffer)` paints a `PaintSurface` holding the
  `CVPixelBuffer`; a view drawn from the last frame repaints last frame's buffer. The
  view has to be handed a new buffer through its entity (`entity.update(..)`, with or
  without `cx.notify()`), which is what Slopty's `ScreenView` does. A buffer read from an
  `Rc<RefCell<..>>`, an atomic or a channel inside `render` would stay stale.
  `crates/gpui/src/fast/tests/surface.rs` checks both ways and the parent-notified case.
- **Presentation reports** (`Window::on_frame_presented`) are per window, reported by the
  renderer for every scene it presents, retained or not. Nothing to fix.
- **Insets, input modality, a11y**: an insets change and an input modality change refresh
  the window; accessibility turns retention off. Nothing is drawn stale.
- **`request_animation_frame` from paint** (the terminal's cursor blink): it notifies
  the view for the next frame, which is then built again and asks again.
- **`CursorStyle::None`, the outline style, `paint_glyph_scaled`**: painted into the frame's
  cursor styles and scene, which a retained view's replay copies.

To rule retention in or out, run with `GPUI_VIEW_RETENTION=0`.

### gpui-kit under Retained Mode

`cargo test --no-fail-fast -p gpui-base -p gpui-component -p gpui-kit --lib` in our
gpui-kit fork, on this fork: gpui-base 1239 passed and 5 failed, gpui-component 575
passed and 1 failed. With `GPUI_VIEW_RETENTION=0`, only the two tests counting a cached
view's builds fail. Each failure reads or writes something Retained Mode does not see:

- `input::state::tests::test_input_does_not_invalidate_cached_parent_during_paint` and
  `text::window_selection::tests::selection_inside_a_cached_view_survives_replayed_frames`
  count how often a cached view is built. gpui-fast builds a cached view again when an
  entity it read was written, and the input or selection inside it writes its own state
  while it is drawn: more builds than upstream, the same pictures. With retention off
  every view is built on every frame, so they fail there too.
- `virtual_list::tests::{horizontal,vertical}_visible_range_and_deferred_scroll_are_preserved`:
  `VirtualListScrollHandle::scroll_to_item` keeps the request in its own `Rc<RefCell<..>>`
  and notifies nobody, so the list's view is drawn from the last frame. The handle has to
  change the `ScrollHandle` it wraps (which bumps its version) or notify the view.
- `text::state::tests::reveal_range::a_reveal_that_cannot_be_shown_gives_up`: a reveal is
  counted by what an `Inline` reports from its prepaint through an `Arc<Mutex<..>>`; a
  block drawn from the last frame reports nothing, so the reveal waits for its timeout
  instead of giving up after a few frames.
- `scrollbar::tests::repeated_touch_and_mouse_drags_keep_the_painted_grab_point`
  (touch, vertical): the thumb painted after a touch drag moves back to its start is the
  one painted 5 px further, not yet explained.

## Measured

`gpui_perf --auto` on mac-studio (Apple silicon, macOS 27, window at 60 fps), at
`751acaf` (zed bd747337, longbridge ac1c226, our patches). Main-thread CPU per frame
(p50) and instructions per frame:

| Scenario | Retained | From scratch (`GPUI_VIEW_RETENTION=0`) |
| --- | --- | --- |
| Spinner | 1.18 ms, 8.8M | 2.00 ms, 15.5M |
| Scrolling the sidebar | 1.30 ms, 9.8M | 2.12 ms, 16.2M |
| Scrolling a page | 1.40 ms, 10.2M | 2.12 ms, 16.5M |
| Scrolling the table | 1.27 ms, 8.0M | 2.04 ms, 14.4M |
| Scrolling the list | 0.85 ms, 5.2M | 1.51 ms, 11.0M |

The `RefreshTable` scenario drew no frames here, retained or not. `gpui_perf --headless
--verify` painted the same quads retained and from scratch in all 25 simulated scenarios.

## Syncing

### From longbridge/gpui-fast

```sh
git fetch origin
git merge origin/main
script/check-upstream && cargo test -p gpui --features test-support
```

If longbridge imported a zed of its own since, both histories hold a `zed: import` merge
and the tracked directories conflict. Keep whichever zed is newer: if ours, resolve the
tracked files to ours and keep their `fast/` changes; if theirs, take theirs, keep
`UPSTREAM` pointing at their vendor commit, and move `zed-vendor` to it. In both cases
put our hooks and patches back and rerun `script/check-upstream`.

### From zed

Per [docs/upstream-sync.md](docs/upstream-sync.md) "Syncing with upstream":

```sh
git switch zed-vendor                     # at UPSTREAM's import_commit
# replace every directory UPSTREAM tracks with zed's copy at <sha>
git -C <zed checkout> archive <sha> <tracked dirs that exist at <sha>> | tar -x
git add -A <tracked dirs> && git commit -m "zed: import <short sha>"
git switch main && git merge zed-vendor
```

Then port by hand what the merge cannot see, update `UPSTREAM` (`zed_commit`,
`import_commit`, `tracked`), add zed's new workspace dependencies to `Cargo.toml`, and run
`script/check-upstream`, `cargo test -p gpui --features test-support` and
`cargo clippy -p gpui -p gpui_perf --all-targets --features gpui/test-support -- -D warnings`.

Code gpui-fast copied from upstream, which the merge never conflicts on:

| Upstream code | gpui-fast's copy | bd747337 sync |
| --- | --- | --- |
| `crates/gpui/src/bounds_tree.rs` | `crates/gpui/src/fast/bounds_tree.rs` (`#[path]` redirect) | unchanged upstream |
| `Window::paint_glyph` in `window.rs` | `Window::paint_glyph_in_run` in `fast/glyphs.rs` | atlas key passed by value (#64331), ported |
| `ViewElement`'s `Element` bodies in `view.rs` | `fast/retained.rs` | upstream split them into helpers without changing behaviour; nothing to port |
| `request_layout` bodies in `taffy.rs` | `fast/layout.rs` | unchanged upstream |
| per-size native fonts in `gpui_macos/src/text_system.rs` | `gpui_macos/src/fast/text_system.rs` | unchanged upstream |

### Conflicts to expect

In the bd747337 sync:

- `window.rs`: upstream's inspector gating (#64309) duplicated gpui-fast's
  (`fast/global_id.rs`); upstream's is kept. Upstream's per-paint-range debug selectors
  (#64433) join `PaintIndex` beside gpui-fast's window control hitboxes, and
  `PaintIndex::shifted` in `fast/retained.rs` has to carry every new index.
- `entity_map.rs`: the access and update hooks sit in `read_inner` and `lease_inner`.
- `element.rs`: the global id cache and layout key hooks sit in `prepare_element_id`.
- `line_layout.rs`: `LineLayoutIndex` gained `font_generation`; `fast/text.rs` shifts it, a
  frame after `TextSystem::add_fonts` is drawn from scratch
  (`Window::refresh_if_fonts_changed`), and a text measurement is carried over only
  within one font generation.
- `view.rs`, `elements/text.rs`: take upstream around gpui-fast's forwarding bodies and
  visibility bumps.

For our patches: `window.rs` (`paint_glyph_scaled` sits next to gpui-fast's
`pub(crate) fn should_use_subpixel_rendering`), the root `Cargo.toml` (`gpui_ios` member
and dependency) and `Cargo.lock`. `git am -3` needs the preimage blobs from the zed fork;
write them into this repository first (`git -C ../zed-main cat-file blob <sha> | git
hash-object -w --stdin` for every `index` line of `format-patch --full-index`).

## Building inside Slopty's checkout

Under `slopty/.research/`, Cargo also reads Slopty's `.cargo/config.toml`. Its
`IPHONEOS_DEPLOYMENT_TARGET` makes `aws-lc-sys` (a dev-dependency through
`reqwest_client`) reject the host compiler, and Slopty's `rustfmt.toml` asks for nightly
options. Build and format with:

```sh
IPHONEOS_DEPLOYMENT_TARGET= cargo test -p gpui --features test-support
cargo fmt -- --config-path /path/to/empty/rustfmt.toml
```
