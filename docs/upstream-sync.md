# Keeping upstream files upstream

gpui-fast is a fork of GPUI, the UI framework in Zed's `crates/gpui`, together
with the Zed crates it depends on. Zed keeps changing those crates, and we want
to take their changes with a merge that mostly resolves itself. That only works
if our changes to upstream's files stay small: a file we rewrote conflicts on
every sync, a file with a one-line hook almost never does.

So gpui-fast's code lives in files upstream doesn't have, and upstream's files
only hold the hooks that call into it.

## The rules

1. **gpui-fast's logic lives in `crates/<crate>/src/fast/`.** One file per
   topic: `crates/gpui/src/fast/retained.rs`, `fast/dependencies.rs`,
   `fast/layout.rs`, and so on. A new topic gets a new `fast/<topic>.rs`, or
   `fast/<topic>/` when it needs more than one file. Other upstream crates get
   their own `src/fast/` when they need one. Tests of what we add go in
   `crates/gpui/src/fast/tests/<topic>.rs` or at the bottom of the topic's file.
2. **Upstream files only hold small hooks, and every hook names `fast`:**
   - one field holding the topic's state, typed as a struct defined in `fast/`
     (`pub(crate) fast_layout: crate::fast::layout_key::WindowLayout`) and
     initialized by its path (`crate::fast::layout_key::WindowLayout::default()`);
   - a one-line call into `crate::fast::...`, or a method whose body only
     forwards to one. The call names the path even where a method would read
     shorter: `crate::fast::dependencies::note_notify(&mut self.entities, id)`,
     not `self.entities.note_notify(id)`, and
     `crate::fast::dependencies::StateVersion::bump(&state.version)`, not
     `state.version.bump()`. Whoever merges upstream can then tell every line
     of ours from upstream's at a glance;
   - a visibility bump (`fn` to `pub(crate) fn`) so a `fast/` module can reach
     upstream's items. Methods of upstream types can be defined in an
     `impl Window { ... }` block inside a `fast/` file for `fast/` code to
     call, but an upstream file calls a free function in `fast/` instead, so
     the call site shows where the code lives;
   - where no path fits — a field, parameter or local of a plain type that a
     hook threads through upstream code — an identifier named `fast_...`
     (`fast_layout_key: u64`);
   - `mod` lines;
   - a `#[path = "fast/<file>.rs"] mod <name>;` redirect when we replaced a
     whole upstream file with our own rewrite. The upstream file then stays
     exactly as upstream has it, unused.
3. **No new types, algorithms, bookkeeping or tests in upstream files.** No
   reformatting, reordering or renaming of upstream code either: code we don't
   need to change stays byte-for-byte upstream's.
4. **Name fast code by its full path.** Outside `fast/`, write
   `crate::fast::<topic>::Name` where it is used, never a
   `use crate::fast::…` line, so every hook shows where its code lives. The
   one exception is `gpui.rs` exporting a `test-support` item one at a time
   (rule 5). Inside `fast/`, listing names in a `use` line is fine. Glob
   imports or re-exports of `fast` (`pub use fast::*`,
   `use crate::fast::layout::*`) are never allowed.
5. **No new public API.** gpui-fast changes how GPUI draws, not what it offers:
   its public API is upstream's. What tests and `gpui_perf` need to measure or
   switch retained mode is compiled only under `test-support`, and exported
   from `gpui.rs` one item at a time:
   `#[cfg(any(test, feature = "test-support"))] pub use fast::stats::LayoutStats;`.
6. **New files only inside `fast/`, or in our own crates** such as
   `crates/gpui_perf` (benchmarks, examples and the frame-measuring app) and
   `compat/` (the `gpui-pre-*` stand-ins GPUI Kit applications patch in).
   Documentation goes in `docs/`.

Which directories are upstream's is recorded in [`UPSTREAM`](../UPSTREAM) at
the repository root: every directory in `crates/` except `gpui_perf`, and
`tooling/perf`.

## Checking

```sh
script/check-upstream                   # compare with upstream as imported into our history
script/check-upstream --zed ~/github/zed  # compare with a zed checkout at the recorded commit
script/check-upstream --report          # print the table without failing
script/check-upstream --all             # also list the changed files that pass
```

The script compares every file in the upstream directories with upstream's
version of it: by default `git show <import_commit>:<path>` from our own history,
or with `--zed` the same path in a zed checkout at `zed_commit`. It checks the
working tree, so run it before committing. It fails when:

- a file outside `src/fast/` is added to, or removed from, an upstream
  directory (`LICENSE*` files excepted);
- a hunk of a changed upstream file adds more than 8 lines (`--max-hunk`);
- a changed upstream file adds more than 40 lines (`--max-added`) or removes
  more than 20 (`--max-removed`);
- a hunk of a changed upstream `.rs` file adds lines none of which names
  `fast`: a path such as `crate::fast::...`, `mod fast;` or
  `#[path = "fast/..."]`, or an identifier starting with `fast_`. One mention
  covers the hunk, since rustfmt may spread one call over several lines. Hunks
  that only remove lines, or only change `use` declarations or blank lines,
  are exempt;
- a line added to an upstream `.rs` file calls a method on a value named
  `fast_...` (`self.fast_layout.end_frame()`): the name marks the hunk but
  hides which module the method is in, so the hook calls it by its path,
  `crate::fast::layout_key::WindowLayout::end_frame(&mut self.fast_layout)`;
- a binary file differs;
- any file glob-imports from `fast` (`use ...fast::*`,
  `use ...fast::<topic>::*`), or a file inside `fast/` has a glob import of
  any kind, `use super::*` in a test module included;
- a file outside `fast/` has a `use` of `fast` at all (`use crate::fast::…`,
  `pub(crate) use …`): only a public `pub use fast::…` export passes.

Apart from the glob rules, files under any `src/fast/` directory are never
checked. A line that differs from upstream's only by a visibility bump
(`pub(crate)`, `pub(super)`) is a hook by definition: it is counted in the
table's `pub(crate)` column and not against any budget.

A justified exception goes in `script/upstream-allowlist`: one line per path or
glob, optionally raising the budgets (`hunk=N`, `added=N`, `removed=N`),
letting that many hunks not name `fast` (`unmarked=N`, for a feature a fork
of gpui-fast keeps in the upstream file it extends rather than a hook), or
allowing any change (`any`), always with a reason. The usual reasons are a hub
file with many one-line hooks, and an upstream body replaced by a `fast/`
implementation it now forwards to:

```text
crates/gpui/src/view.rs removed=210 # ViewElement's cache-by-bounds is replaced by fast::retained's retained views
```

Removed lines deserve the most care: upstream's changes to code we deleted
conflict on every sync, and have to be ported into `fast/` by hand.

### Where our API differs from upstream's

The check cannot see a change to what upstream's public types offer, so the
few places where gpui-fast's differs from upstream's are listed here. Each is
forced by what `fast/` keeps; anything not listed here is upstream's API
unchanged, and a new entry needs as good a reason.

- `ViewElement`'s `Element::RequestLayoutState` and `PrepaintState` are
  `fast::retained::ViewLayoutState` and `ViewPrepaintState`, opaque types, in
  place of `Option<AnyElement>`. `ViewElement` is `#[doc(hidden)]`, and the
  states are only ever handed back to it by GPUI.
- `Scene` has a public `layers` field (`fast::layers::scene::SceneLayers`),
  and `gpui` exports `LayerKey`, `TileCoord`, `SceneLayers`, `LayerFrame`,
  `LayerContent` (a layer's content, in parts: one per row of a virtual
  list), `LAYER_TILE_TEXTURE_BASE`, `layer_tile_texture_id`, `layer_tile_id` and
  `decode_layer_tile`, because renderers live in other crates and read scroll
  layers from the scene. Polychrome sprites whose texture index is at or
  above `LAYER_TILE_TEXTURE_BASE` are scroll layer tiles, not atlas textures.
- `crates/gpui/Cargo.toml` names this repository and sets `publish = false`.
- `Window::paint_keyed(key, origin, paint)` is new public API, the one
  addition that is not forced (`fast/keyed.rs`; docs/retained-mode.md,
  "Keyed paint"). An element that paints itself — a terminal's grid, a
  chart, a code view — builds nothing retention could compare, so it painted
  every frame whole: a 200 × 60 terminal paints its 12 000 cells again for a
  blinking cursor. With it the element names each stretch it paints, a row
  say, by a key standing for everything the stretch paints relative to an
  origin, and a stretch whose key was painted last frame is copied from last
  frame's scene, in place or moved by whole device pixels. It is opt-in,
  paints exactly what painting afresh paints (the oracle test
  `fast/tests/keyed.rs`), and takes nothing of GPUI's that upstream lacks
  but the scene replay retention already has. Measured on Slopty's terminal
  element, 200 × 60 with its rows keyed, in instructions per frame:
  unchanged 3.12M → 0.37M (−88%), a blinking cursor 3.17M → 0.43M (−86%),
  one row changing 3.53M → 0.80M (−77%), a line of output a frame 4.43M →
  1.87M (−58%), every cell changing 25.01M → 25.07M (+0.2%). In
  `gpui_perf`, `strip-scroll-keyed` takes 7.45M against `strip-scroll`'s
  7.97M (−6.5%).
- `PlatformInput::Gesture(PlatformGesture)`, with `RotateEvent`,
  `SmartMagnifyEvent` and `SwipeEvent` and the `InteractiveGestures`
  listeners (`on_rotate`, `on_smart_magnify`, `on_swipe`), are new public API
  (`fast/gesture.rs`, `gpui_macos/src/fast/gesture.rs`, and `gpui_ios`'s
  rotation recognizer). Upstream delivers a trackpad's pinch and turns a
  swipe into a back or forward button; Slopty forwards gestures to a remote
  desktop, so it needs a rotation, a smart magnify and a swipe as they came.
  A swipe that no listener stops still becomes the back or forward button,
  so nothing that relied on that changes. On macOS a rotation is AppKit's
  degrees counterclockwise since the previous event, with its phase; a
  smart magnify is the two-finger double tap; a swipe carries AppKit's
  `deltaX`/`deltaY` of ±1 and arrives `Ended`. On iOS a rotation from
  `UIRotationGestureRecognizer`, touch screen or trackpad, is delivered in
  the same units; UIKit has no smart magnify, and iPadOS keeps
  three-finger trackpad swipes for itself.
- `CursorStyle::Image(CursorImageId)`, `CursorImage`, `CursorImageId`,
  `CURSOR_IMAGE_MAX_SIDE` and `App::set_cursor_image` are new public API
  (`fast/cursor.rs`, `gpui_macos/src/fast/cursor.rs`), with a defaulted
  `Platform::set_cursor_image`. Upstream's cursors are the system's named shapes; a
  remote desktop shows the far side's pointer, which only a picture can be. Drawn by
  the view, that pointer trails the hand by a frame on every move; as the system
  cursor it moves with the hand. Platforms other than macOS keep the arrow for it.
- `gpui_platform::text_system()`, with `gpui_macos::text_system()` and
  `gpui_ios::text_system()`, is new public API (`gpui_platform/src/fast/text_system.rs`,
  `gpui_macos/src/fast/text_system.rs`): the platform's text system made without the
  platform. Upstream's only way to the Mac's text system is the platform, which
  panics off the main thread, and a test never runs on the main thread; Core Text
  needs none, so a test that shapes and rasterises real glyphs (Slopty's terminal
  paint oracle) takes it from here. Windows' and Linux's text systems need their
  platform's devices and still come from a headless platform.
- `Window::paint_mask`, `RenderMaskParams` and `AtlasKey::Mask` are new public API
  (`fast/mask.rs`). Upstream's monochrome sprites are glyphs and SVGs, and an SVG is
  rasterised at twice the device size and halved by the sampler. Slopty's icons are the
  operating system's symbols, drawn by the system at the size they are shown; halving
  them after costs crispness, most at 1x. A mask is the caller's bytes, one alpha byte
  per device pixel, painted at exactly that size with its origin rounded to a device
  pixel, and rasterised once per key and size (`fast/tests/mask.rs`). A transformation, as
  `paint_svg` takes, turns it while it moves (a disclosure chevron); at rest it is the unit.
- `Window::render_to_image_at(scale, cx)` (test-support, `fast/render_at.rs`) and the
  defaulted `PlatformWindow::fast_render_scene_to_image`, forwarded by the macOS window to
  its renderer's offscreen render, are new. Upstream renders a window only at its display's
  scale; Slopty's visual tests run on a 1x display and keep one Retina golden, so a
  regression that shows only at 2x is caught too.
- `TextSmoothing`, `App::set_text_smoothing`, `App::text_smoothing`,
  `Window::with_text_smoothing` and `Window::text_smoothing` are new public API
  (`fast/text_smoothing.rs`). Upstream dilates light glyphs on macOS as Core
  Graphics' font smoothing does, with only the user's `AppleFontSmoothing`
  default to turn it off; an application designed in weights, as on the web
  with `-webkit-font-smoothing: antialiased`, needs every colour drawn at the
  weight it is set in. `Native`, upstream's behaviour, is the default.
- On macOS and iOS a face is matched on the CSS weight AppKit gives its Core
  Text weight (`gpui_apple::fast::font_weight::css_weight`, public for
  `gpui_ios`), not font-kit's, so `FontWeight(500)` is a family's Medium face
  rather than its Regular. The call's signature is upstream's; what it
  returns for 500 and 800 differs.
- `edge_fade(child, EdgeFade)`, `EdgeFade`, `EdgeFadeElement`,
  `Window::with_edge_fade`, `EdgeFadeRamps` and `Scene::edge_fades` are new
  public API (`fast/edge_fade.rs`, `gpui_apple/src/fast/edge_fade.rs`; docs/
  retained-mode.md, "Edge fades"). Upstream can fade content at a clipped
  edge only by painting a gradient of the background's colour over it,
  which needs an opaque background of one colour; over a window's glass, a
  video or another tile only fading the content itself works. The index a
  primitive's fade is kept under goes in the padding every primitive
  already has: `pad` in a shadow, an underline and the three sprites, and
  `Background`'s padding, now `pub(crate)`, in a quad and a path. The
  primitives keep upstream's layout and size. Only Metal draws the fade;
  the wgpu and Direct3D renderers draw everything unfaded. With
  `test-support`, `Scene::edge_fade_ramps`, `Scene::add_edge_fade` and
  `Scene::insert_faded_primitive` let a renderer's test build a faded scene.
- A tooltip (`.tooltip(..)`, `.hoverable_tooltip(..)`) on an element that
  tracks a focus handle also shows while the element is focus-visible:
  focused, with the keyboard the last input (`fast/focus_tooltip.rs`).
  Upstream shows tooltips to the pointer alone, and counts no element
  hovered after keyboard input, so a control reached with Tab never names
  itself. The tooltip shows after the element's delay, centred below the
  element or above it at the window's foot. It hides when the focus leaves
  or the pointer is used, and on Escape, which a keystroke interceptor
  takes before any binding sees it, keeping the focus where it is. An
  element with a tooltip pays one test in its paint and, with a focus
  handle, one focus question; `focus_tooltip_bench` in
  `fast/tests/focus_tooltip.rs` measures 100 such controls.
- `StateTransition` and `StatefulInteractiveElement::transition` are new
  public API (`fast/transition.rs`). Upstream swaps a hover, active, focus or
  drag-over style in the frame its state changes; a design system's pointer
  states that come in at once and leave over 150 ms, as Linear's and
  Raycast's do, need the colours eased. Upstream's only way is an
  `AnimationElement` the application restarts from a hover listener, which
  rebuilds the element every frame and still cannot start from the colour
  shown when the state changes again. The transition follows only the
  background and border colours paint uses, so layout never waits on it, an
  element's own colour changing is applied at once, and
  `App::reduce_motion` turns it off. Unused, it is one empty pointer in
  `Interactivity` and one test in its paint (`transition_bench` in
  `fast/tests/transition.rs`).
- `ValueTransition`, `ValueTransitionState`, `Window::use_keyed_transition`
  and `Window::use_transition` are new public API
  (`fast/value_transition.rs`), ported from gpui-ce's `Transition`
  (<https://github.com/gpui-ce/gpui-ce>, `crates/gpui/src/transition.rs`,
  the duration-based form before its `Motion` rewrite; Apache-2.0, the
  gpui-ce contributors; the notice is kept at the top of the file).
  Upstream's `with_animation` restarts from its start, and `with_spring`
  wraps one element; neither gives a surface one value, eased over a
  duration, that every property it draws can follow and that turns back
  from where it stands. Changed from gpui-ce: the executor's clock, so tests
  advance it; the value shown when the goal changes is taken at that moment
  rather than from the last frame drawn; a turn back takes only the time
  the way back covers, as CSS transitions shorten a reversal;
  `App::reduce_motion` applies every change at once; a changed goal
  notifies the view holding the value. No upstream code calls it, so it
  costs nothing unused; a hundred values at rest add about 0.02 ms to a
  frame (`value_transition_bench` in `fast/tests/value_transition.rs`).

When the check fails, move the change into a `fast/` module and leave a hook
behind that names it; use `git diff <import_commit> -- <file>` to see what
differs.

## Syncing with upstream

Upstream is [zed-industries/zed](https://github.com/zed-industries/zed). The
commit our copy was taken from is `zed_commit` in `UPSTREAM`, and
`import_commit` is our commit holding that copy unchanged. To take a newer zed:

1. In a zed checkout, pick the new commit and run
   `script/import-upstream --zed <checkout> <commit>`. It starts a vendor
   branch at `import_commit` and replays each zed commit since `zed_commit`
   that touches a directory listed in `UPSTREAM`, limited to those
   directories and keeping its author, dates and message, so zed's history
   of those crates comes along. The branch holds nothing but upstream's code;
   its last commit is the new vendor commit.
2. Merge that branch into ours. Conflicts should only touch hooks; resolve them
   by keeping upstream's code and putting our hook back. The pull request must
   be merged with a merge commit, not squashed: the next sync's merge needs
   the vendor commits in `main`'s history.
   Name our sync commit `chore: Sync GPUI upstream <zed-commit>`, using the
   short target Zed commit, for example `chore: Sync GPUI upstream a1b71072e5`.
   Do not start its title with `Merge`; replayed vendor commits keep their
   original upstream messages.
3. For every file we redirect with `#[path = "fast/..."]`, look at what
   upstream changed in the original (`git diff <old vendor commit> <new vendor
   commit> -- <file>`) and port it into our copy by hand. The merge won't
   conflict on those files, so this step is easy to forget.
4. Update `zed_commit` and `import_commit` in `UPSTREAM` to the new zed commit
   and the new vendor commit, and add or remove entries under `tracked` if zed
   added or removed crates we use.
5. Run `script/check-upstream`, the tests and clippy.
