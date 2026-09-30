# Retained mode

How gpui-fast draws a frame from the last one, what an application needs to
know about it, and how it is checked and measured. The code is in
`crates/gpui/src/fast/`: `retained.rs` (retained subtrees), `dependencies.rs`
(what a subtree read), `layout.rs` and `layout_key.rs` (retained layout nodes)
and `stats.rs` (counters for tests and benchmarks). The reasoning behind the
design is in [`architecture.md`](architecture.md).

## How it works

A frame walks the element tree three times: **request_layout** renders views
and asks for layout, **prepaint** computes layout and places elements,
**paint** turns them into the scene handed to the GPU. Upstream GPUI normally
does all three from scratch every frame, except where an explicitly cached
view is reused.

### Views

Every view — any `Entity<V: Render>` or `AnyView` placed in the element tree,
cached or not — is a retained subtree. While nothing it depends on changed
since the last frame, it is not rendered, laid out, prepainted or painted: its
layout comes from the nodes it kept, and its hitboxes, listeners, dispatch
nodes and primitives are copied from the last frame. A view depends on:

- **What it read.** Every entity accessed and every global read while it was
  rendered, laid out, prepainted and painted — its own entity, models it reads
  without observing them, the views nested in it — and the `ListState`s and
  `ScrollHandle`s its elements track. Those two bump a version whenever their
  state changes, so a view is drawn again when one it read moved, notified or
  not. A view that only asked whether a global is set (`cx.has_global::<G>()`)
  depends on that alone: setting the global where it was not, or removing
  it, changes it; writing to it does not. Reading the window's pointer
  position (`window.mouse_position()`) or its modifier keys and caps lock
  (`window.modifiers()`, `window.capslock()`) is recorded the same way, and
  an input event that changes them draws again the views that read them.
- **What it was updated with.** An entity updated (`entity.update(..)`) while
  no view is being drawn — by a task, a listener, an action — and notified
  counts as changed for every view that read it. So does an entity notified
  while a view is being drawn. An entity updated without being notified — as
  every `cx.subscribe` or `cx.observe` handler updates its subscriber, whether
  it cares about the event or not — counts as changed only for views drawn
  inside a view notified since the last frame. Upstream builds those again
  with everything under them, and a view often changes a model it renders and
  notifies only itself.
- **Where it is drawn.** Its bounds, content mask, text style and opacity. A
  view that moved is built again, at the layout nodes it kept, and laid out at
  the size its parent gave it.
- **The hovers it was painted by**, and interactions inside it: a hover,
  scroll or press that changes how it looks draws it again. Only the
  innermost view it happened in is built again; the views around it are
  drawn from the last frame around it, as around a notified view.

A view that asked for an animation frame (`window.request_animation_frame()`)
is built again on the next frame drawn, as if notified, without waiting for
the frame callback that notifies it: what it drew was for the frame it was
drawn in.

Nothing is drawn from the last frame while the window is being refreshed
(`window.refresh()`, and what refreshes it: a resize, a focus change), on the
first frame after fonts were added (`TextSystem::add_fonts`), while something
is dragged, while the inspector is picking, or while accessibility is active.

A notified view marks the views around it dirty, because they have to be
walked to reach it. A view that is dirty only for that reason — it was not
notified, nothing it read itself changed and it is hovered as it was — is not
built again: it is drawn from the last frame stretch by stretch, with the
nested views that changed built again in the gaps where they were, at their
own layout nodes and with what they inherited there. If a nested view asks for
another layout, the view around it is built after all, taking over the nested
view already built rather than building it twice. A view whose own reads
changed — an entity updated without being notified, a global written, a
scroll or list state moved — is marked dirty as if notified, so the views
around it are drawn from the last frame around it rather than built again
because something nested in them changed. The more of a window is split into
views, the less a change costs. The code is in
`crates/gpui/src/fast/splice.rs`.

A view counts as having read itself, so an application that changes a view
outside drawing (`entity.update(..)`) and notifies it gets it built again on
the next frame, with every view that read it. Changed without being notified,
it is built again only when a view around it was notified. Conversely, an
entity notified without being updated — as a scroll wheel, a dragged
scrollbar or an animation notifies a view to draw it again — is built again
itself, but a view that read it is not: nothing it holds has changed. An
application that changes what an entity holds through interior mutability
(`entity.read(cx).cell.borrow_mut()`) has to change it with `update` instead
for the views reading it to see it. A frame driver or timer that only needs
to notify another view should notify it by id (`cx.notify(entity_id)`) rather
than updating the view that owns the driver.

The window asks the focused text input things every frame — whether it
accepts text, its selection, the bounds of a range — through
`ElementInputHandler`. Those calls update the input's entity, but do not
count as changing it unless it notifies while it is asked.

### Elements

A view that changed is rendered again and builds every element it holds, but
most of them are usually built just as they were. Once built, each `div`,
`svg` and piece of plain text is compared with the element built at its place last
frame — found by its layout key, as its layout node is — and one built the
same way, everything nested in it included, is drawn again from last frame:
its layout nodes are kept rather than requested, and its dispatch nodes and
primitives are copied. What it was built with is compared exactly: a `div`'s
style refinement, element id and children, an `svg`'s style refinement,
element id, path or bytes and transformation, text's text. Where it is drawn is
compared too: bounds, content mask, opacity, text style and rem size. An
element that differs is built as upstream builds it, but the elements nested
in it are compared on their own, so a quote row whose price changed builds
the row and the price and draws its other cells from last frame.

Only elements whose output is fully decided by what they were built with take
part: a `div` or `svg` with a listener, focus, scroll, hover or active style,
a tooltip, a group or a cursor, a `div` holding any other kind of element, and
an `svg` drawn from a file (`external_path`, which draws once the file has
loaded, with nothing in the element changed), are built as upstream builds
them, and so is everything around them, though what is nested in them can
still be drawn again. Accessibility roles and properties do not count: the
accessibility tree is built only while an assistive technology is attached,
and element retention is off then. Text a layout measured again this frame is
built again, since it may break into other lines at the same size.

An element nested in no other that is recorded is recorded only once it is
drawn twice in a row in the same place, or moved by a whole number of device
pixels on two frames in a row, and one nested in a recorded element that
moves otherwise is recorded only as moving: rows below a line inserted once
above them would not be drawn again from last frame, and are not compared
and recorded for nothing, while rows under a scroll are drawn again moved
(see [Elements drawn again moved](#elements-drawn-again-moved)). Where it
stood last frame is checked first, before anything nested in it is compared,
so a moving root costs one lookup rather than a walk of its subtree. A root that cannot take
part itself (a row holding a button) still keeps its place from frame to
frame, so the plain cells inside it are recorded as standing still and drawn
again on their own.

An element built differently on each of two frames in a row, with nothing
nested in it drawn again either (a price ticking in its cell), rests: it is
drawn as upstream draws it, neither compared nor recorded, for one frame,
then two, doubling up to sixteen while it keeps changing. After a rest it is
recorded again and compared on the next frame, and once it is drawn again,
or holds anything that is, it starts over. Only frames in a row count: an
element drawn again in between, even as part of a larger one, never rests. The records of such an element
and of those nested in it are frozen, once it is painted, into one subtree
shared from frame to frame, which drawing it again takes over as it is. The
code is in `crates/gpui/src/fast/element.rs`.

`GPUI_ELEMENT_RETENTION=0` turns this off, leaving view retention on.

Element retention came as longbridge/gpui-fast#17 (`fbeb4f9`); this fork adds
to it, to reconcile with whatever longbridge lands:

- `svg` elements take part (`Snapshot::Svg`), and accessibility fields no
  longer make a `div` ineligible. Covered by
  `an_svg_showing_another_image_is_drawn_anew` and the element oracle's icons
  and roles.
- Probation is looked up before the subtree is walked, and an ineligible root
  keeps its placement (`Phase::Ineligible`). Covered by
  `plain_elements_beside_an_interactive_one_are_reused`.
- Text kept from a frame that wrapped it is measured unwrapped when Taffy
  probes it with no width, rather than answering with the wrapped size
  (`text_let_out_of_the_box_that_wrapped_it_is_measured_unwrapped`). #17
  made this reachable: a node kept across frames is probed again under
  constraints its kept measurement was not taken with.
- Records keep their ranges as `u32` offsets from their root's
  (`PrepaintAt`, `PaintAt`), 304 bytes rather than 568.
- Elements that change every frame rest (`Rest`). Covered by
  `an_element_built_anew_every_frame_rests`,
  `an_element_drawn_again_between_its_changes_does_not_rest` and the element
  oracle's ticking quote.
- `gpui_perf` has a Slopty-shaped screen (`strip-*`: tiles of terminals with
  headers, icons and a status bar) and counts elements built and reused.

### Elements drawn again moved

An element whose only change since last frame is where it is drawn, moved by
a whole number of device pixels at the same size — a row under a scroll, a
tile in a strip sliding sideways, rows below rows inserted at the head of a
list — is drawn again from last frame too: its records are taken over with
their bounds moved, and its primitives are copied moved. The code is in
`crates/gpui/src/fast/shift.rs`, and `element.rs` decides when it applies.

Moving what an element painted has to give exactly what painting it afresh
at the new place gives, so a move is refused, and the element painted
afresh, unless:

- It moved by whole device pixels, so everything snapped to device pixels or
  quantized to glyph subpixel steps lands on the same steps, moved. Every
  place it paints is a multiple of 1/64 of a device pixel, where moving adds
  without rounding; the side of a border drawn around a rounded corner of a
  fractional radius is not, and is painted afresh.
- Along the axes it moves, the element, everything nested in it and every
  glyph it placed lie at or past the window's top and left edges before and
  after: coordinates round half toward zero, which rounds the same way only
  on one side of zero.
- What it is clipped by is known: either the content mask moved with it,
  and every mask inside it too, or the mask stood still (a list's viewport)
  and every mask inside it lies clear of its edges, and nothing it left out
  for lying outside the mask comes into it with the move, which is checked
  against the side and the distance it lay beyond.
- No primitive leaves its mask with the move, and no paint layer reaches the
  mask's edge, where a clip can't be told from the edge.
- It holds only primitives: no hitbox, listener, input handler, cursor
  style, tooltip, deferred draw, path, surface or native view, whose places
  are held by code the move can't reach.

What a primitive can't tell — where each glyph was placed before rounding,
and what was left out for lying outside a mask — painting notes as it goes,
into each record. Noting costs every element painted, so it is done only
while an element is in motion: from the frame a root, or anything nested in
it, moves, for 30 frames after it last did. An element painted outside that
is noted as unknown and is never moved; the first frame of a motion paints
afresh what moves, and draws it again moved from the second frame on. A
move is worked out once, while the element is prepainted, into a list of
moved operations its paint inserts as they are.

`LayoutStats::elements_moved` counts the elements drawn again moved.

### Keyed paint

An element that paints itself, rather than building children — a
terminal's grid, a chart, a code view — builds nothing that could be
compared, and paints every frame whole. `Window::paint_keyed(key, origin,
paint)` lets it name a stretch of what it paints, a row say, by a `u64` key
that stands for everything `paint` paints relative to `origin`. The code is
in `crates/gpui/src/fast/keyed.rs`.

- A stretch whose key was painted last frame is not painted: its paint
  operations are copied from last frame's scene. Where the origin, the
  content mask, the opacity, the paint layer and the scale factor are all as
  they were, it is copied as it is. Where only the origin moved, by whole
  device pixels, it is copied moved, under the rules for an element drawn
  again moved; painting a stretch always notes what moving it needs.
  Otherwise `paint` is called.
- The key is the caller's promise. Two stretches under one key paint the
  same primitives relative to their origins, including every texture they
  paint that can leave the atlas between frames. Keys are one space per
  window: a key found anywhere in last frame is taken, which the promise
  makes sound, and an element whose keys must not meet another's folds in
  what tells them apart.
- A stretch paints primitives only: quads, glyphs, sprites, underlines,
  shadows, paint layers and content masks. One that registered a listener,
  a hitbox's input handler, a cursor style, a tab stop or a window control,
  read element state or laid out text is painted every frame, because
  drawing it again would leave those out. A stretch that pushed a content
  mask of its own is not moved under a mask that stood still, since its own
  mask can't be told from the one around it.
- A view or element drawn again whole carries the keyed stretches it holds
  into the new frame, so an element painted every few frames still finds
  them. A stretch that painted nothing, all of it culled, is carried only
  from inside what was drawn again, never from its ends, where it may be of
  what was painted beside it.
- With retention off, or while the window refreshes, every stretch is
  painted.

`LayoutStats::paints_keyed` counts the stretches painted, `paints_replayed`
those drawn again, and `paints_moved` those of them drawn again moved.

### Records per retained subtree

Each frame keeps a record per retained subtree: where its hitboxes, dispatch
nodes, listeners and primitives went, what it read, the hovers it was painted
by and the layout nodes it holds. The records live in the frame rather than in
element state, because the stretches they point to belong to one frame. Drawing
a subtree again copies its record and the records nested in it, shifted to
where the copy landed, so a nested subtree stays reusable on its own later,
when what is around it has to be built again. A view that is built again
therefore does not build everything nested in it.

### Cached views

`Entity::cached(style)` and `AnyView::cached(style)` are upstream's API and
work as upstream documents them. They are retained subtrees like any other
view, so they are also built again when an entity or global they read changed,
and keep their layout nodes while they are reused. A notified cached view is
built again on its own, at the layout node it kept, inside the views around
it drawn from the last frame, as any nested view is.

### Layout nodes

Upstream clears the whole Taffy tree at the end of every frame. gpui-fast keeps
nodes from one frame to the next, and writes a node's style, children and
measurement only when they differ from last frame's, so Taffy's own layout
cache survives and unchanged parts of the tree are not laid out again. An
element finds its node again by:

- its path from the root of the element tree, each step being the element's
  `ElementId`, or its position among siblings without one;
- its `ElementId`, wherever it moves among its siblings;
- its index, for an item of a `uniform_list` or `list` without an `ElementId`,
  so rows still in view keep their layout while the list scrolls.

A node unclaimed for a frame is released.

A text element measures itself, and a measured node given a new closure would
be dirtied every frame, with every node above it. Instead, when last frame's
text element at the same place measured the same text, runs and text style,
the new element takes a copy of that measurement and the node is left clean.
A view built again, because it moved or because the view around it was
notified, is then not laid out again unless something in it changed.

Text that did change — a price ticking in a table cell — is measured again,
but not by Taffy. Each measured node keeps the constraints Taffy measured it
under since it was last dirtied, and the size each gave. The new text is
measured under the same constraints, in the same order; when every size comes
out the same, what Taffy cached for the node and every node above it still
holds, and the node is left clean. Only text whose size changed dirties its
row, its list and the window above it.

## What an application needs to know

Nothing, as long as what a view's render reads lives in entities, globals,
list or scroll state, or the window's pointer position, modifier keys and caps
lock, which are tracked too. Anything else it reads — an `Rc<RefCell<..>>`
shared outside entities, the time — it has to be notified of (`cx.notify()`),
as a cached view already has to be in upstream GPUI. Otherwise it keeps
showing what it showed when it was last built.

What still costs a rebuild every frame is a change made every frame. An
entity notified, or a global written (`cx.global_mut`, `cx.update_global`),
while the window draws — in prepaint or paint — or on every frame, counts as
changed even when the value is the same, and every view that read it is
built again on the next frame, which makes it write again. A resizable panel
that notifies its state on every prepaint keeps every view reading that state
from being retained. Notify or write only when the value changes.

To rule retention in or out when something looks stale, run with
`GPUI_VIEW_RETENTION=0`: every view is then drawn from scratch each frame, as
upstream does.

## How it is verified

A retained frame has to be the frame drawing from scratch would have produced.

- The oracle test (`crates/gpui/src/fast/tests/oracle.rs`) drives two windows
  through the same random history, one drawing incrementally and one from
  scratch, and requires every frame to match. It covers sibling, nested and
  deferred views notified alone, a model read without being observed, and a
  global, and asserts that views really were reused.
- `crates/gpui/src/fast/tests/element_oracle.rs` does the same for elements
  drawn again inside views rendered every frame: keyed and unkeyed rows
  inserted, removed and moved, components, interactive elements among plain
  ones, inherited text styles, opacity and clips, scrolling, focus and
  deferred draws, comparing the dispatch tree, listeners and focus too.
- `crates/gpui/src/fast/tests/shift.rs` drives the same pair of windows
  through moves: rows under a scroll, past the window's top and left edges,
  in a clip that moves with them and in one that stands still, boxes and
  lines sliding through a clip, moves by part of a device pixel, and rounded
  borders that can't be moved, requiring every frame to match and moves to
  have happened where they can.
- `crates/gpui/src/fast/tests/keyed.rs` drives the pair through stretches
  painted under keys, re-keyed, added, dropped and moved at random, whole or
  by part of a device pixel, under clips that move or stand still, in paint
  layers, with clips of their own and listeners, and requires every frame
  and the listeners to match and stretches to have been drawn again, moved
  or not. `gpui_perf`'s `strip-output-keyed` and `strip-scroll-keyed` verify
  it on a terminal strip.
- `crates/gpui/src/fast/tests/retained.rs` covers reuse, rebuilding when a
  dependency or a hover changes, moved views and retention turned off.
- `cargo run -p gpui_perf --release -- --headless --verify` compares the quads,
  text, icons and images painted with retention on and off on every frame of
  every simulated screen.

```sh
cargo test -p gpui --features test-support
cargo run -p gpui_perf --release -- --headless --verify
```

## How it is measured

Measure release builds only. [`CONTRIBUTING.md`](../CONTRIBUTING.md#measuring)
has the full usage.

`cargo run -p gpui_perf --release` opens the showcase: a component gallery
shaped like GPUI Kit's, which scrolls its sidebar, a page or a data table by
itself, refreshes the table on a timer, and shows in its status bar what each
frame costs. `--auto` runs each of those with retention on and off and prints
the comparison:

```sh
cargo run -p gpui_perf --release
cargo run -p gpui_perf --release -- --auto
```

With `--headless`, `gpui_perf` drives simulated screens — forms, lists, a
data table, a settings page — without a window, with real text shaping, with
retention on and off, and compares what each frame cost:

```sh
cargo run -p gpui_perf --release -- --headless
cargo run -p gpui_perf --release -- --headless --scenario table --frames 200
```

The `views_frames` example draws a dashboard of panel views into a real
window; the arguments are panels, labels per panel and panels changed per
frame. It prints main-thread CPU per frame and the build, prepaint and paint
times:

```sh
cargo run -p gpui_perf --example views_frames --release -- 60 64 2
GPUI_VIEW_RETENTION=0 cargo run -p gpui_perf --example views_frames --release -- 60 64 2
```

A headless benchmark of a 100-row quote board rendered every frame, with
element retention on and off, is in
`crates/gpui/src/fast/tests/element_bench.rs`:

```sh
cargo test -p gpui --lib --release element_bench -- --ignored --nocapture
```

A headless benchmark of 60 panel views × 64 labels is in
`crates/gpui/src/fast/tests/retained_bench.rs`:

```sh
cargo test -p gpui --lib --release retained_bench -- --ignored --nocapture
```
