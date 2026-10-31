# What this fork carries that longbridge/gpui-fast lacks

Every patch below is in `aislopware/gpui-fast` and not in `longbridge/gpui-fast`. This was
checked on 2026-10-12 against longbridge main `231f2cd` (#62): each patch's code was looked for
in longbridge's tree, and the five newest were also read against longbridge's own versions of
the same functions. Nothing here has been offered upstream. Publishing is the person's call, and
this file is meant to let them decide it in one pass.

Each entry gives a verdict:

- **Fix, wanted.** It fixes code longbridge has. Each entry names the test that shows the
  failure. For `7bc5da8`, `b535227` and `2a63006`, the test was run against the code without
  the fix and failed. For the others, the commit message records the failure.
- **Fix, with its pair.** It is correct on its own, but it only shows up next to another
  patch of ours, so the two go up together.
- **Performance.** It keeps the drawn result the same and costs less, with the measurement
  that justifies it.
- **Feature.** It adds something any GPUI application could use. Whether upstream wants it is
  a product question rather than a correctness one.
- **Fork-only.** It serves Slopty's platforms or choices, or longbridge already has its own
  answer. Not a candidate.

Commits are listed by their hash on the fork's `main`. A test name points to a test in
`crates/gpui/src/fast/tests/` unless a crate is named.

## Fixes to code longbridge has

### A list's offset set without a notification (`7bc5da8`). Fix, wanted

Since longbridge #56, `ListState::set_offset_from_scrollbar` leaves the list state's version
alone, so that a dragged scrollbar composites from the layer. The offset is then taken in only
once some scroll container decides on its layer. Code that sets the offset without notifying
the view holding the list gets that view drawn from the last frame on the next frame: a bounce
animation, a test that draws, a scrollbar whose host does not notify. The list stays where it
was, and its wheel listener goes on scrolling from the old offset. Offsets set since the last
frame are now taken in as the frame begins. The view that laid out the scrolled list is built
again as if notified, and its list still composites from its layer. This is what made GPUI
Kit's four `scroll_bounce` list tests fail (-50 where -1000 was expected). Test:
`layers_lists::offset_set::a_list_offset_set_without_a_notification_shows_and_scrolls_on`.

### Hover held still only for what the wheel moves (`b535227`). Fix, wanted

Longbridge #57 holds hover still while the wheel scrolls under a still pointer by emptying the
hit test. That unhovers everything around the list too: a sidebar panel with a hover style is
built again on the first frame of every scroll, for a hover that did not change. The freeze
also ends with `window.refresh()`, which builds every view in the window. Now, of what was
hovered when the wheel began, whatever has not moved stays hovered, and nothing the wheel
brings under the pointer is. The list's foretold hit test holds the same rule. The end of the
freeze hit-tests again and marks the window dirty, so only what hovers differently is built
again. Slopty's navigator panel showed the bug (Slopty's
`a_scroll_of_the_navigator_draws_its_rows_alone`). Test: `layers_lists::rows::hover_held_still_keeps_the_hover_of_what_the_wheel_does_not_move`.
Longbridge's own `hover_held_still_while_the_wheel_scrolls_renders_no_row_for_it` keeps
passing, with "nothing is hovered" narrowed to "no row is hovered".

### A node a lingering parent takes back (`2a63006`). Fix, wanted

`TaffyLayoutEngine::apply_children` skips `set_children` when a node's kept children equal the
new ones. A list row's node lingers between frames. If its child was meanwhile given to another
parent (as the key of a view root drawn in its place), Taffy had detached it from the lingering
parent. When the row came back with the same child, the child was never listed under it again,
so it was never measured. Text in it then panicked in prepaint with "measurement has not been
performed on main". This happened with GPUI Kit's Questionnaire inside a list row. The skip now
also requires each child's Taffy parent to be this node. Test:
`layout::a_node_taken_back_by_a_lingering_parent_is_laid_out_under_it`.

### A cached view updated under a notified view (`ecf73e4`). Fix, wanted

Longbridge #48's `notified_dependencies_changed` draws a cached view again only for what
notified it. It leaves out an entity updated outside drawing without a notification, which
every other retained view counts when it sits under a notified view. A listener that hands a
cached view something and notifies only the view around it (a dropped file handed to a
composer) shows nothing until the cached view is next notified. Such updates count again under
a notified view. #48's purpose stays intact: a write made while the window draws still does not
redraw a cached view. Test: `retained::a_cached_view_updated_under_a_notified_view_shows_the_update`.

### Writes kept when unioning dependencies (`3b88067`). Fix, with its pair `a62f45e`

`RenderDependencies::union` returns `self` when the other side read nothing, which drops the
writes the subtree made while the other side was recorded. A view holding a list reads nothing
of its own in prepaint, so its rows' writes fall out of its own writes. With `a62f45e`'s
`note_rebuild`, those writes then count as foreign, and the layer renders every shown row on
each scrolled frame. On longbridge's own masking (`rendering_since`) the loss does not show,
so this goes up with `a62f45e`. `Writes::union` now also merges stretches that meet. Test:
longbridge's own `layers_lists::a_view_and_rows_writing_what_the_rows_read_keep_the_layer`
fails with `a62f45e` and without this.

### A list's holder writing its own state as it is built (`a62f45e`). Fix, wanted

A list's wheel notifies the view holding it, which is built again on each scrolled frame. When
that view renders an input, the input writes its own state as it renders and prepaints (GPUI
Kit's Input writes eight times, Textarea nine). The scroll-only check counted those writes as
foreign, so the list demoted after three frames and never composited again. A subtree's writes
during its own layout and prepaint now count as its own. A write to what it read from outside
still counts. Tests: `layers_lists::a_list_beside_an_input_composites_its_layer` and
`a_write_from_outside_the_holder_renders_its_rows_again`.

### Splicing every out-of-date nested view (`7afb44d`). Fix, wanted

A view that was dirty only because a nested view was notified was spliced with only the
notified views rebuilt. A nested view that read a changed model (updated without a notify), a
global, a scroll state or a hover was copied stale, where upstream GPUI rebuilds every view
under the notified one. The gaps are now the outermost nested records that are out of date
for any reason. A deferred draw, whose views are not nested in the record that deferred them,
refuses the splice when anything it read changed (`deferred_out_of_date`); the retained oracle
found this at seed 14. Spliced records also take their baselines up to the check
(`checked_at`), so the next frame does not find them changed again. Longbridge #10's
`mark_changed_retained_views_dirty` covers the entity, global and state part of the first
fix, but not hovers. The element-state fixes made while merging #10 belong here too: a
spliced view keeps neither its gaps' element states nor the last frame's whole layout. Tests: `a_view_that_read_a_changed_model_is_built_again_beside_a_notified_one`
and `a_view_drawn_around_a_nested_view_built_again_is_reused_on_the_next_frame`.

### A subtree's own scrolls taken as its own (`f8feeb0`). Fix, wanted

A retained subtree recorded each scroll offset it read at the version the state had when it
first read it. A view that reads where its list is scrolled and then scrolls it as it renders
(`scroll_to_reveal_item`, a remeasure) moves that version itself. On the next frame the read
looked changed, and the view was rebuilt whenever a sibling was notified. Offset reads are now
kept at the version the state has as the recording ends. Test:
`a_view_that_scrolls_its_own_list_as_it_renders_is_not_built_again_beside_a_notified_one`.

### A focus move or notify made while drawing (`589f8b4`). Fix, wanted

A view that moves the focus as it renders (a workspace giving the keyboard to a tile) left the
views drawn after it in that frame with the old focus. The focus questions were only asked
before a frame, and the window asked for no new frame. `fast::focus` now asks again when the
focus moves mid-draw, and a splice checks that what it copies still holds once its gaps are
built. Tests: `views_drawn_after_the_focus_moves_while_drawing_show_it_in_that_frame`,
`a_view_drawn_before_the_focus_moves_while_drawing_is_drawn_again`,
`a_render_leaving_the_focus_where_it_was_asks_for_no_frame` and
`a_view_notified_while_drawing_is_built_in_that_frame_when_drawn_after`.

### Focus containment during paint (`c4e9967`). Fix, wanted

An element drawn for the first time inside the focused element was not "within" it during its
first paint. Containment was read from the last frame, which did not hold the element yet, and
nothing asked for another frame. During paint, the frame being painted now answers; its
dispatch tree is complete once prepaint is done. Test:
`an_element_drawn_first_inside_the_focused_one_is_within_it_at_once`.

### A notify from a focus listener (`8e135d4`). Fix, wanted, and for Zed too

Focus listeners run inside the draw. A notify raised there marked the view dirty without
marking the window dirty or waking the platform, so a caret that starts blinking on focus
waited for something else to draw. Such a notify now asks for a frame. Upstream GPUI has the
same bug. Tests: `a_caret_started_by_a_focus_listener_is_drawn`, with and without retention.

### An animation frame request honoured on the next frame (`4d0009e`). Fix, wanted

`request_animation_frame` notifies the view from a callback of the next frame the platform
delivers. A frame drawn before that (for another reason, or by a test calling `Window::draw`)
drew the view from the last frame. An animation stood still, and a popup positioned by what it
measured while drawing never opened. Such a view now counts as notified on the next frame
drawn. GPUI Kit's popup, popover, navigation stack and toast tests depend on it. Test:
`a_view_that_asked_for_an_animation_frame_is_built_again_on_the_next_frame_drawn`.

### Rebuild records holding views weakly (`db34272`). Fix, wanted

A frame keeps, for each view, what it takes to rebuild that view on its own. It held the view
strongly, so a view its owner dropped lived on, with everything it held, until the next frame.
A closed web page kept its process and data store until the window happened to draw again.
Records now hold views weakly; a view that is gone falls back to rebuilding the view around it.
Test: `a_native_dropped_with_its_view_goes_without_waiting_for_a_frame`.

### Sub-pixel moves and a cached view's snapping (`509aa8f`, `f95c1d7`). Fix, wanted

An element was drawn again from the last frame when its snapped bounds matched. A node moved
by part of a device pixel can snap to the same place while a box it centres snaps to the next
pixel. Drawn again, that box stays a pixel off where a fresh paint puts it (a terminal's
sticky header showed its text a pixel high). Records now keep their phase within the device
pixel and are reused only at the same phase. Relatedly, a view laid out on its own (cached)
snapped its content differently from the same view laid out in place: the half-pixel tie went
the other way, and its bounds' sub-pixel offset was lost. Both now match. Tests:
`a_move_its_snapped_bounds_hide_is_painted_afresh`,
`a_view_moved_by_part_of_a_pixel_its_bounds_hide_is_drawn_afresh`,
`a_cached_view_snaps_what_sticks_out_of_it_as_in_place` and
`a_cached_view_placed_off_a_pixels_edge_snaps_as_in_place`.

### Fonts added while running (zed `bd747337` import). Fix, wanted

A text measurement carried over from the last frame stays valid only within one font
generation. When fonts are added (`fast::text::refresh_if_fonts_changed`), every view is
rendered again; otherwise text keeps the metrics of the font it fell back to before the font
arrived. Test: `retained::every_view_is_rendered_again_once_fonts_are_added`.

### Content masks

- **A grown mask (`589f6fd`). Fix, wanted.** An element painted outside motion noted no
  culls and was taken to have culled nothing. When the mask around it grew without it moving
  (a clip switched off), it was drawn again from the last frame without what the old mask had
  culled. An unknown cull now claims every side, which refuses a grown mask and still accepts
  a shrunk one. The randomized element-tree oracle failed on `RootClipping` for three seeds.
  Test: `what_a_clip_left_out_comes_back_when_the_clip_is_switched_off`.
- **A path vertex's unread mask (`4cc51b6`). Fix, wanted.** No renderer reads
  `PathVertex::content_mask`, but the fast layer moved, clipped and hashed it anyway. A
  layer's moved path then carried a zero-size mask where a fresh paint kept it at the origin,
  and the stale-frame oracle called identical frames different. Test:
  `layers_oracle::frames_drawn_through_scroll_layers_match_frames_drawn_without`.
- **Glyph culling by the font's box on the baseline (`7296c50`). Fix and performance,
  wanted.** A line tested each glyph against its mask pushed out by the font's bounding-box
  height above and below the baseline. That is correct on tall lines but lets about a line of
  glyphs past every clipped edge, each costing a sprite lookup (strip-scroll paid 2.6% for
  it). Each run now places the font's box where the font puts it, widened to ascent and
  descent, and works the mask's edges out once per run. Test:
  `a_glyph_may_reach_the_mask_where_its_font_box_overlaps_it`.
- **A line's content mask taken once (`973a34b`). Performance.** Strip-scroll fell from
  +7.91% to +0.02% of instructions against before longbridge #19.

### Scroll layers

- **A list row's scene index on a copy (`e831224`). Fix, wanted.** When a holder's paint was
  copied, the shift subtracted the copied range's start from the row's own scene index. That
  overflows (a debug-build panic) when the holder is painted after more of the window's scene
  than the rows hold. Test: `a_holder_copied_past_its_rows_scene_keeps_its_layer`.
- **Debug bounds of composited content (`e1f7041`). Fix, wanted.** A composited frame
  carried its content's paint records but dropped their debug bounds, so `debug_bounds` of
  anything in a scrolled div or list vanished after two wheel scrolls. Tests:
  `a_composited_layer_keeps_its_contents_debug_bounds_where_they_show` and
  `a_composited_list_layer_keeps_its_rows_debug_bounds_where_they_show`.
- **A surface in a scroll layer (BGRA surfaces). Fix, wanted.** A layer's tiles were
  rasterized from its surfaces too, and kept the picture they were rasterized with once the
  view handed over the next buffer, though the scroll-layer notes said surfaces bypass the
  layer. Surfaces now go to the overlay with the paths. Test:
  `a_surface_is_drawn_over_the_tiles_with_the_picture_last_painted`.
- **A surface's picture in the text frames are compared by. Fix, wanted.** The retained check
  described a surface without its buffer, and `Scene::describe` without the surface at all, so
  a retained frame showing an older picture than one drawn afresh passed. Both now name the
  buffer. Test: `a_frames_description_names_the_buffer_each_surface_shows`.

### Touch and accessibility

- **A fling that moves nothing ends (`03e0051`). Fix, wanted.** A fling ran its whole curve
  whatever it scrolled. A list flung into its end sat still for a second or more, and every
  touch in that time caught the fling, so no tap anywhere in the window registered. Tests:
  `a_fling_into_a_lists_end_stops_there`, `a_tap_beside_a_fling_is_a_tap` and six more.
- **An a11y node reported where it shows (`916ad60`). Fix, wanted.** A node's bounds were its
  whole layout bounds, whatever its ancestors clipped, so VoiceOver framed the wrong place and
  pressed what was drawn there. Test: `a_node_reports_and_is_pressed_where_it_shows`.
- **Focus on the nearest labelled ancestor (`6c13772`). Fix, wanted.** A widget often keeps
  its focus handle on an inner element with no role (GPUI Kit's input does). The tree's focus
  then stayed on the window, and a screen reader announced the window. Tests:
  `a_focused_element_without_a_role_focuses_its_labelled_ancestor` and
  `a_node_less_focus_falls_back_to_the_nearest_ancestor`.

### Platforms

- **Core Text runs shaped at their size (`9c29ff9`). Fix, wanted, and for Zed too.**
  Upstream shapes every other run at `font_size.next_up()` so Core Text would not merge
  adjacent runs. Core Text tracks such a size differently, so lines came out up to 0.35 px
  wider or narrower than Core Text sets them. Runs already reach the shaper merged by face,
  so nothing is left for the alternation to break. Test (gpui_macos):
  `a_line_is_as_wide_as_core_text_sets_it_at_its_size`.
- **CSS weights matched on AppKit's weights (`d7480c0`). Fix, wanted, and for Zed too
  (zed#14175).** font-kit puts every Medium face at 530, so `FontWeight::MEDIUM` drew Regular,
  and 800 drew Black. Tests (gpui_macos): `appkit_weights_are_the_css_weights` and
  `each_css_weight_reaches_its_own_face`.
- **Video textures released on the render thread (`af3714d`). Fix, wanted.** Releasing a
  `CVMetalTexture` in a command buffer's completion handler calls back into a texture cache
  that the render thread flushes concurrently; a streamed window crashed there. Test
  (gpui_apple): `surface_textures_go_on_the_render_thread_once_their_frame_is_done`.
- **Destination alpha blended over (`gpui_apple` pipelines). Fix, wanted, and for Zed
  too.** Longbridge's pipelines blend destination alpha with `One`, so a non-opaque window's
  alpha comes out wrong. Ours use `OneMinusSourceAlpha`, and an opaque window renders
  bit-identically either way. Shown by composition's pixel tests over natives.
- **gpui_apple's build script decided on the target (`bea273f`). Fix, wanted.** A build
  script's `cfg` is the host's, so clippy or rustdoc for an Apple target could not run from
  Linux. No test; it is shown by checking an Apple target from Linux.
- **A headless atlas with one texture per kind (`48b0c1a`). Fix, wanted.** The headless atlas
  gave every tile its own texture, so a headless frame batched sprites one glyph each and
  measured what no window does (strip-clock 1.79 to 1.68 M instructions a frame).

### Compat

- **Stand-ins for GPUI Fast's crates.io release (`6de0b2f`). Fix, wanted.** `compat/` is
  longbridge's. GPUI Kit's `gpui-fast` feature names the crates.io release (`gpui-fast` 0.1.7),
  so a kit patched onto a gpui-fast checkout ran a second, unpatched GPUI under that feature.
  Three more stand-ins carry those names. Checked by CI's compat step and by GPUI Kit's
  `cargo test -p gpui-kit --features gpui-fast,test-support`.

## Performance

Each keeps drawing identical; the oracles compare frames drawn from scratch. Numbers are
`gpui_perf` instructions per frame unless stated. All are wanted where longbridge measures
the same scenarios.

- `4105616`: a finished scene sorted by radix with packed keys. strip-hover -47%,
  strip-clock -42%. Test: `sorting_by_gathering_is_a_stable_sort_by_key`.
- `5030410`: a reused range drawn again by index. strip-clock 2.51 to 1.79 M, strip-hover
  -33%. Test: `replaying_entries_out_of_order_gives_what_inserting_it_would`.
- `fec8c94`: a kind gathered again in last frame's order. strip-hover -11%.
- `fbeb4f9`: unchanged elements inside rebuilt views drawn again from the last frame (element
  retention). Tests: `unchanged_elements_of_a_rendered_view_are_reused` and
  `text_measured_again_is_not_drawn_again`.
- `fbbb9f5`: a moved element drawn again, translated. layout-rows-at-head-keyed -41.3%,
  list-uniform-scroll -16.3%.
- `71f4488`: a custom element's keyed stretches drawn again. A 200 x 60 terminal unchanged
  3.12 to 0.37 M (-88%).
- `a56199d`, `02adbe7`, `3b1c671`, `7ca0e59`, `bc80d7a`: rest for elements rebuilt every
  frame, u32 record ranges, skipping moving roots, no noting outside motion, a smaller replay
  budget after motion. Each 2 to 12% on the scenarios named in its message.
- `beb580e`: only the views whose focus answers changed are rebuilt. strip-focus -14.9%.
  `22f237a`: the tab order sorted when read.
- `817e345`: four decoration runs kept in a shaped line, not 32. GPUI Kit's 50,000-line editor
  scrolls 15% cheaper.
- `08a34bc`, `71c1d4c`: a layer's tile scenes built in one walk, and tiles rasterized a few a
  frame. On a 40-tile page, 72 M to 3.5 M instructions for the scenes, and 7 to 10 ms to
  2.3 ms on the frame a scroll begins.
- `b6d9f08` (gpui_apple): bind only what the previous batch left different. -11% to -15% of
  encoding at 34 to 82 batches. Test: `binding_only_what_changed_draws_what_upstream_draws`.
- `7f3613f` (gpui_apple): frames no longer queue in front of the display. Animation reached
  the glass after about 31 ms, and now after about 18 ms. `e825e6b` (gpui_macos): a window's
  display link stops while it wants no frames.
- `35a88ca` (gpui_apple): what an opaque native covers is neither drawn nor cleared. Fork-only
  until composition is shared (see below).

## Features

Generic, each with tests in its commit, and each a product question for upstream:

- `2e199c6` `ScrollEvent::momentum_phase`: momentum told apart from a finger and a wheel.
  A candidate for Zed itself.
- `a8868ac`: trackpad rotate, smart magnify and swipe events.
- `67fb314` `CursorStyle::None`; `032a19b` and `5cd6369`: a cursor with the application's own
  picture.
- `c5f0200`: an outline style, a ring drawn clear of the element.
- `9c128c2` and `602eef5`: an element's colours eased into and out of its states.
- `e828ae7`: an element's tooltip shown to keyboard focus too. `9e1f84c`: a hover tooltip
  anchored centred below or above its element rather than at the pointer
  (`TooltipPlacement`), the next tooltip shown at once within 400 ms of the last (a delay
  group), and `aria_disabled`; the anchored tooltip kept 8 px inside the window.
- `198c79f` and `5fdd4f3`: an element as an accessibility live region.
- `d797f55`, `a03d04e` and `d216a97`: content faded per pixel toward a region's edges.
- `67c4d96`, `c54b1ea` and `35e1e61`: `Window::paint_mask`, a caller's mask at exact device
  pixels, rotated as an SVG is.
- `2d3df6a`: a text smoothing policy, antialiased or the platform's.
- `28d2801`: a window drawn offscreen at another display scale.
- `38cbd2b`: when each frame reaches the display (`Window::on_frame_presented`).
- `2dd0ff6`: the platform's text system without the platform.
- `d550137`: the clipboard on a pasteboard the application names, not always the general one
  (`gpui_platform::set_clipboard_pasteboard`; macOS and iOS).
- `5d79996`: the accessibility tree exposed to tests. `dfb5ad0`: the test window records
  where the platform is asked to put the input method. `1776aa2`: a retained-mode test that
  a video surface shows the buffer its view holds.
- `ea73091`, `a33830d` and `cb82e62` (gpui_apple): 10-bit and 4:4:4 video surfaces, each
  surface sampled by its own Y'CbCr matrix, and video presented through a `VideoLayer`.
- BGRA surfaces (gpui, gpui_apple): an `IOSurface`-backed BGRA buffer drawn as it lies, with
  straight alpha, at its element's opacity and in its edge fade, so an offscreen renderer
  hands GPUI the buffer it drew with no read-back and no atlas upload.

## Fork-only

- `crates/gpui_ios` and its commits (zed#63068 ported and extended): an iOS platform crate
  longbridge does not track. `ae1ef6b` (the simulator's drawables without presented handlers)
  belongs with it and with `38cbd2b`.
- Window composition (`280f369`, `fb0b0e0`, `c9d5968`, `bbd12d8` and the rest): longbridge has
  its own (#3398). Ours differs and stays here.
- Ports of Zed pull requests (`66bae85`, `057969e`, `da4e350`, `65891f9`, `1cc5051`):
  longbridge takes them from Zed.
- `abd688f` and `1a933ba`: superseded by longbridge #56 (lists replay their rows).
- Ports from zui (zeronsh/zui, Apache-2.0) for Slopty's copy of zeron's UI: `e3540f1` a
  list's tail reservation; `4cc6457` rounded clips; `1cb5741` and `d7b5523` backdrop blur and
  its shared pass breaks; `54e69ce` image alpha masks; `d763dcf` transformed and blurred
  glyphs; `8ec4028` `Window::on_present`. They serve zeron's look in zui's API shapes.
  `dc23ac0` (`ImageSource::evict`, `App::peek_asset`) fixes a leak longbridge shares: each
  image shown from bytes keeps its atlas tiles after `remove_asset`. It could be offered as a
  fix.
