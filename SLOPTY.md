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
  longbridge commit merged is `f6e82b4` (#13, "keep carried lines in the line cache, and
  measure text in fast/"), in `4c13f16`; before it `ac1c226` (#12), in `751acaf`.
- longbridge's open PR #10 ("keep views retained in a real GPUI Kit application",
  branch `retained-real-apps`, head `fd23405`) is merged ahead of longbridge, in the
  commit "Merge longbridge/gpui-fast#10 (fd23405) into Slopty's fork". When longbridge
  merges it, merging `origin/main` again brings nothing new from it; if they squash it,
  expect the files below to conflict and resolve them to what is here.
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

Merged ahead of longbridge:

- longbridge/gpui-fast#10 at `fd23405` (18 commits), merged whole. What Slopty needs most
  from it is `Entity::query` (`fast::dependencies::query`, crate-private): the window asks
  the focused input handler `accepts_text_input`, its selection and range bounds after
  every paint through `ElementInputHandler`, and that no longer counts as updating the
  view, so a focused terminal or screen view is not built again on every frame drawn.
  Measured with a focused text field beside a spinner notified on every frame, 60
  frames: the field was built 120 times before the merge (twice a frame), 0 after
  (`a_focused_text_field_is_not_built_while_a_view_beside_it_animates`).
  It also changes the rules ([docs/retained-mode.md](docs/retained-mode.md)): an entity
  updated without a notify counts as changed only for views inside a view notified since
  the last frame, `has_global` depends only on whether the global is set, a view whose
  own reads changed is marked dirty so the views around it are spliced, hovers and
  scrolls build only the innermost view, cached views can be splice gaps, a view around
  a gap laid out differently is still spliced when its own nodes hold, and text,
  paths, fonts and scene ordering got faster. How it was adapted:
  - Conflicts: our zed bd747337 sync against its "every hook names `fast`" rewrite
    (`entity_map.rs`'s `read_inner`/`lease_inner`, `element.rs`'s
    `prepare_element_id`/`prepare_inspector_id`, the inspector gating kept as upstream's
    #64309, `PaintIndex::debug_bounds_index` in `PaintIndex::shifted` and its `PartialEq`,
    `LineLayoutIndex::font_generation` in its `PartialEq`/`Debug`,
    `crate::fast::text::fonts_changed()` beside `add_fonts`' font generation), and our
    splice fixes (`Splice` keeps both `checked` and `held`).
  - Left out: `e0137e8` ("gpui_wgpu: record a frame with one upload, kept bind groups
    and fewer passes", `crates/gpui_wgpu/src/fast/`). It is written against the
    `WgpuRenderer` before zed split it into `WgpuRendererCore` (bd747337), and Slopty
    renders with Metal only. `wgpu_renderer.rs` stays zed's.
  - Fixes found by zed's `randomized_element_tree` tests, which came with bd747337 and
    which longbridge has not run yet. With #10 a root updated without a notify is
    spliced rather than built, which exposed that a spliced view kept every element
    state its gaps had used: the layout's own (`RetainedLayout::element_states`) and
    those a view built at the layout it kept (`build_at_retained_layout`) used during
    the prepaint of the view around it. Both are now left out by element id prefix
    (`fast::splice::inside_any`), and a spliced record's layout is rebuilt from the kept
    part and the gaps' new layouts, where it used to keep last frame's whole. Test:
    `a_spliced_view_lets_go_of_the_element_states_its_gaps_no_longer_use`.
  - Tests changed for the new rules: `a_view_that_read_a_changed_model_is_built_again_beside_a_notified_one`
    (was `…updated_without_a_notify…`: the model is now notified), the surface test's
    buffer handed over without a notify (shown once a view around it is notified), and
    zed's `randomized_entities_and_work_counters_support_incremental_benchmarks` no
    longer asserts the root is rendered for a child's change (a removal-only hunk).
    New: `an_update_without_a_notify_passed_over_by_a_splice_counts_once_inside_a_notified_view`.
  - `script/check-upstream` gained `unmarked=N` in the allowlist: #10 fails any hunk of
    an upstream file that doesn't name `fast`, and our patches are features, not hooks.
    Each file with such hunks has its count in `script/upstream-allowlist`. Our own
    hooks that didn't name it (`note_animation_frame_request`,
    `refresh_if_fonts_changed`) became `crate::fast::…` calls.

Ported from open zed pull requests, ahead of zed. Drop each at the zed import that
brings it: take zed's version of the files, remove the pull request's line from
`script/upstream-allowlist`, and keep only what is listed here as ours.

- zed #64239 (@huacnlee, head `3e74364c02`), "a touch that catches a fling picks its own
  axis": a finger that stops a sideways fling and moves up scrolls up, where it used to
  inherit the fling's axis and scroll nothing. `gestures.rs` only, applied as is. iOS is
  covered: `gpui_ios` hands raw touches to the same `TouchGestureRecognizer`. Test:
  `catching_a_fling_takes_the_axis_from_the_new_touch`.
- zed #63469 (head `943c6c15e2`), "size-dependent glyph padding": a glyph's raster bounds
  get a left margin of `ceil(font_size × 0.03 × scale)` device pixels, 1 to 5, and 1 on the
  other sides, because CoreGraphics draws a size-specific font whose ink reaches farther
  left than font-kit's bounds. Applied as is to `gpui_macos` with its two tests
  (`test_system_zero_raster_bounds_do_not_clip`, `test_system_zero_matches_appkit`, run
  with `--features font-kit`). At 14 pt and 2× the margin is 1 px, as before.
  - Ours, kept when the import drops the macOS part: the same bounds in `gpui_ios`
    (`ios/text_system.rs`), which had no margin at all. Its test
    `glyph_raster_bounds_do_not_clip` rasterizes a bold tabular `0` at 12 to 48 pt and 3×
    and compares the ink with a buffer 8 px larger all round; without the margin it fails
    at 24 pt. It runs on the simulator (see "Testing gpui_ios on the simulator").
- zed #64624 (@timvermeulen, head `54c44986d4`), "UAX #14 line breaking": `wrap_line`
  and `compute_wrap_boundaries` break where `icu_segmenter`'s line segmenter allows,
  in place of `is_word_char`. Wrapped lines no longer start with `/` or `?`, emoji
  sequences such as 👍🏽, 🇯🇵 and 👩‍💻 stay whole, and a word may break after a hyphen.
  Applied with its tests. Two adaptations:
  - Its `test_wrap_line_break_opportunities` gained the `IndentAdjustment` argument
    zed's `wrap_line` took after the pull request's base. The expected boundaries
    in `test_extra_columns_overflow_guard` changed from `9, 11, 13, 15` to
    `9, 11, 12, 14, 15`: with 2 columns left after the indent, the words `ef` and
    `gh` stay whole and each space between them wraps alone. The old numbers came
    from `prev_c` going stale across a returned boundary, which split `ef`.
  - Ours, kept when the import brings the pull request: `fast::line_breaks`. The
    segmenter costs about 8 ns a byte, three times the cost of measuring the line. So
    both wrappers ask for break opportunities only when something on the line passes
    the wrap width, and they read ASCII as Latin-1, which gives the same breaks
    (test `ascii_breaks_as_its_utf8_does`). Tests for the exact-fit edge:
    `a_wrapped_line_that_fits_exactly_is_not_broken`,
    `a_shaped_line_that_fits_exactly_is_not_broken`.
  - Cost (`cargo run -p gpui_perf --example wrap_cost --release`, best of 40 rounds,
    2000 lines of 40 to 300 bytes at 320, 560 and 900 px):

    | | before | after |
    | --- | --- | --- |
    | `wrap_line`, mixed scripts | 5.07 ms | 15.3 ms |
    | `wrap_line`, ASCII | 5.56 ms | 13.7 ms |
    | `wrap_line`, every line fits | 1.73 ms | 2.01 ms |
    | `shape_text`, 100 lines | 0.61 ms | 1.61 ms |

    That is about 2.5 µs per wrapped line of 190 bytes, against 0.85 µs before.
    `__TEXT` grows by 48 KiB (the example: 5,865,472 to 5,914,624 bytes). Its four
    new crates compile in about 3 s of CPU in a debug build; the ICU crates they
    share with `idna` were already built.
- zed #64542 (@madcodelife, head `21ad132982`), "text decorations across wrapped
  lines": an underline or strikethrough that continues onto the next wrapped line
  resumes where that line's alignment starts it, not at the box's left edge. The
  last line's end is found the same way. Applied as is to `line.rs` with its test
  `test_wrapped_decorations_follow_text_align`.

Added in this fork:

- `1776aa2` test(gpui): a video surface shows the buffer its view holds, retained or not
- `4d0009e` gpui: build a view that asked for an animation frame on the next frame drawn
- the commit after `4c13f16`: a spliced view builds every nested view that is out of
  date, not only the notified ones, and leaves its record up to date as of the splice

### Candidates for longbridge

Generic to gpui-fast, not to Slopty, and worth a pull request to longbridge/gpui-fast:

- the zed bd747337 sync itself: `2db56fa` (`zed: import bd747337`) and its merge `acfc6db`,
  including the font generation handling (`fast::text::refresh_if_fonts_changed`, a text
  measurement carried over only within one font generation, and its test
  `every_view_is_rendered_again_once_fonts_are_added`);
- `4d0009e`, a view that asked for an animation frame built on the next frame drawn;
- `1776aa2`, the retained-mode test of a surface's buffer (macOS; the surface element is
  upstream's).
- the splice fixes in `fast/splice.rs` (the commit after `4c13f16`), all in `fast/`:
  - A view dirty because a nested view was notified was spliced with only the notified
    views built again. A nested view that read a model updated without a notify (or a
    global changed, or a state version bumped, or a hover) was copied from the last frame
    and drawn stale, where upstream builds every view under the notified one. The gaps
    are now the outermost nested records that are out of date by any of those, and a
    view is spliced when it is not reusable, not only when it is in `dirty_views`.
  - A deferred draw is copied with the view that deferred it, but the views it draws are
    not nested in that view's record. `Window::deferred_out_of_date` refuses the splice
    when anything the view read, less what the gaps read, changed or is dirty. The
    oracle found this (seed 14, step 40) once the first fix spliced more often.
  - A spliced record kept the `updates` and `generation` baselines of the frame it was
    first built in, so the next frame found it changed by updates it had already been
    checked against, and built the whole chain again. `RenderDependencies::checked_at`
    moves the baselines up to where they stood when the splice was checked.
  - Tests: `a_view_that_read_a_changed_model_is_built_again_beside_a_notified_one`
    and `a_view_drawn_around_a_nested_view_built_again_is_reused_on_the_next_frame` in
    `fast/tests/retained.rs`; each fails without its fix.
  - longbridge#10 marks a view whose own reads changed dirty
    (`mark_changed_retained_views_dirty`), which covers the first fix for entities,
    globals and states, but not for hovers; the gap test stays as it is. `checked_at`
    and `deferred_out_of_date` are still needed with #10.
- the element-state fixes made while merging longbridge#10 (above): a spliced view keeps
  neither its gaps' element states nor last frame's whole layout.
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
  view has to be handed a new buffer through its entity and notified
  (`entity.update(..)` with `cx.notify()`). Since longbridge#10 an update without a notify
  shows only once a view around it is notified. Slopty's `ScreenView::show` stores the
  buffer without notifying, so its caller has to notify for every frame once Slopty is
  on this fork. A buffer read from an `Rc<RefCell<..>>`, an atomic or a channel inside
  `render` would stay stale. `crates/gpui/src/fast/tests/surface.rs` checks both ways and
  the parent-notified case.
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

Our gpui-kit fork's `cargo test --no-fail-fast -p gpui-base -p gpui-component -p gpui-kit
--lib` failed 6 tests on this fork before the fixes below. Each failure was something
drawn that Retained Mode did not see change, or a write that it did:

- **Writes while drawing.** gpui-kit updated entities and wrote globals on every frame
  without changing anything: the window selection state's frame bookkeeping and its
  snapshots, the active selection scope, the touch UI bounds, the text view state stack,
  the selection document order and the selection state registry. gpui-fast counts such an
  update or write as a change, even for a cached view, so the view that read them (the
  `Root` among others) was built on every frame. gpui-kit now keeps that bookkeeping in
  `Cell`s and writes only what changed. `selection_inside_a_cached_view_survives_replayed_frames`
  counted 16 builds where 8 were due; a test ticking a sibling of a cached panel of
  Markdown and an input now builds only the sibling.
- **A notify in the first paint.** An input reports its geometry in its first paint and
  notifies; upstream drops that notify (the window has not tracked the entity yet),
  gpui-fast honours it. `test_input_does_not_invalidate_cached_parent_during_paint` settles
  that frame before counting.
- **`VirtualListScrollHandle::scroll_to_item`** kept its request in its own
  `Rc<RefCell<..>>` and notified nobody. It now moves the `ScrollHandle` it wraps, which
  bumps its version, using the geometry of the last prepaint.
- **Text reveal and hit testing.** A `TextView` ran its reveal progress and started its
  selection frame from the element's paint, in the parent's view; when only the state's
  view was built again, or only the parent, the report was lost or the hit test runs
  cleared. Both now run in the state's own view (`TextViewContent`).
- **Scrollbar drag.** A throttled drag defers its notify to a trailing timer; the test's
  handle keeps its offset in plain `Rc` state, so a frame drawn before the timer showed
  the last thumb. The test lets the throttle deliver before it checks the thumb. Real
  handles (`ScrollHandle`, `ListState`) carry a state version.

With these fixes and this fork, the same command passes both ways: gpui-base 1246,
gpui-component 576 (gpui-kit has no lib tests), with retention on and with
`GPUI_VIEW_RETENTION=0`.

Two limits remain. The selection document order is counted per painted participant, so
a partial frame orders only those it painted, as upstream's cached views already did. A
streaming Markdown message grows its layout, so the views around it are built again
("gap layout changed"); relaying the gap out in place when its size holds would keep them.

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
  (`fast::text::refresh_if_fonts_changed`), and a text measurement is carried over only
  within one font generation.
- `view.rs`, `elements/text.rs`: take upstream around gpui-fast's forwarding bodies and
  visibility bumps.

In the longbridge f6e82b4 merge (`4c13f16`): `TextMeasureInputs::new` went away, so the
font generation moves into `fast::text::layout_text` and `shapes_as` compares it;
`LineLayoutCache::finish_frame` keeps our lock order (current frame, then previous) and
the font-generation clear ahead of `carry_over_line_layouts`.

For our patches: `window.rs` (`paint_glyph_scaled` sits next to gpui-fast's
`pub(crate) fn should_use_subpixel_rendering`), the root `Cargo.toml` (`gpui_ios` member
and dependency) and `Cargo.lock`. `git am -3` needs the preimage blobs from the zed fork;
write them into this repository first (`git -C ../zed-main cat-file blob <sha> | git
hash-object -w --stdin` for every `index` line of `format-patch --full-index`).

## Testing gpui_ios on the simulator

`gpui_ios`'s tests that need UIKit or CoreText build for `aarch64-apple-ios-sim` and run
inside a booted simulator. `core-video` pulls in `cgl`, which links `OpenGL.framework`,
absent on iOS; a test binary links against a stub whose install name is a library the
simulator has (`/usr/lib/libobjc.A.dylib`), since nothing calls into it:

```sh
mkdir -p /tmp/iosstub/OpenGL.framework
printf -- "--- !tapi-tbd\ntbd-version: 4\ntargets: [ arm64-ios-simulator ]\ninstall-name: '/usr/lib/libobjc.A.dylib'\n...\n" \
  > /tmp/iosstub/OpenGL.framework/OpenGL.tbd
CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUSTFLAGS="-C target-cpu=apple-m1 -C link-arg=-F/tmp/iosstub" \
  cargo test -p gpui_ios --features test-support --target aarch64-apple-ios-sim --lib --no-run
xcrun simctl boot <device>
xcrun simctl spawn <device> target/aarch64-apple-ios-sim/debug/deps/gpui_ios-<hash>
xcrun simctl shutdown <device>
```

## Building inside Slopty's checkout

Under `slopty/.research/`, Cargo also reads Slopty's `.cargo/config.toml`. Its
`IPHONEOS_DEPLOYMENT_TARGET` makes `aws-lc-sys` (a dev-dependency through
`reqwest_client`) reject the host compiler, and Slopty's `rustfmt.toml` asks for nightly
options. Build and format with:

```sh
IPHONEOS_DEPLOYMENT_TARGET= cargo test -p gpui --features test-support
cargo fmt -- --config-path /path/to/empty/rustfmt.toml
```
