//! Frames that draw elements again from the last frame must match the frames
//! drawn with nothing retained.
//!
//! Each run drives two windows holding the same view through the same random
//! history. The view renders a board of elements — plain divs and text,
//! components, keyed and unkeyed siblings, interactive elements among them,
//! and sections that inherit a text style, an opacity or a clip, scroll, or
//! are deferred — and is notified nearly every frame, so that its elements
//! are built again and compared with last frame's. The other window forgets
//! everything it retains before every frame. Every frame, the two must paint
//! the same primitives in the same places and leave the same hitboxes,
//! dispatch tree, listeners and focus.
//!
//! A spacer above the board, a margin beside its clipped section and its
//! scroll move what they hold, often by whole device pixels, for elements to
//! be drawn again moved (see [`crate::fast::shift`]), in a mask that moved
//! with them or one that stood still.

use std::{borrow::Cow, sync::Arc};

use rand::{Rng as _, SeedableRng as _, rngs::StdRng};

use crate::{
    AnyElement, App, AssetSource, Bounds, Context, DevicePixels, Entity, FocusHandle, Font, FontId,
    FontMetrics, FontRun, GlyphId, Hsla, InputEvent as _, IntoElement, LineLayout, MouseMoveEvent,
    NoopTextSystem, Pixels, PlatformTextSystem, Render, RenderGlyphParams, RenderOnce, Result,
    Role, ScrollHandle, SharedString, Size, SvgRenderer, TestAppContext, TextRenderingMode,
    Transformation, Window, WindowHandle, anchored, deferred, div, hsla, point, prelude::*, px,
    radians, size, svg,
};

const WORDS: [&str; 8] = [
    "a",
    "price",
    "101.25",
    "-0.5%",
    "a longer label",
    "x",
    "volume 1200",
    "a label long enough to wrap in a narrow cell",
];

/// Icons, each named by its own source, which [`Icons`] serves as the svg
/// it names.
const ICONS: [&str; 3] = [
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><rect x="1" y="1" width="6" height="3"/></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><circle cx="4" cy="4" r="3"/></svg>"#,
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"><path d="M0 8L4 0L8 8z"/></svg>"#,
];

/// Serves every path as the svg source it is.
struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Some(Cow::Owned(path.as_bytes().to_vec())))
    }

    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

const PALETTE: [Hsla; 5] = [
    hsla(0.0, 0.0, 0.1, 1.0),
    hsla(0.6, 0.7, 0.5, 1.0),
    hsla(0.3, 0.6, 0.4, 1.0),
    hsla(0.0, 0.8, 0.6, 1.0),
    hsla(0.1, 0.9, 0.5, 0.5),
];

#[derive(Clone, Copy, Debug)]
struct Item {
    id: u64,
    word: usize,
    color: usize,
    width: f32,
    background: bool,
    /// A hover style, which takes it out of what can be drawn again.
    hover: bool,
    focusable: bool,
    /// Rendered by a component rather than inline.
    component: bool,
    nested: bool,
    hidden: bool,
    invisible: bool,
    /// An svg icon beside the label: from an asset, from bytes, turned.
    icon: bool,
    /// An accessibility role and label on the label, which draw nothing.
    role: bool,
}

impl Item {
    fn new(id: u64) -> Self {
        Item {
            id,
            word: id as usize % WORDS.len(),
            color: id as usize % PALETTE.len(),
            width: 40. + (id % 4) as f32 * 30.,
            background: id.is_multiple_of(3),
            hover: id.is_multiple_of(7),
            focusable: id.is_multiple_of(11),
            component: id.is_multiple_of(2),
            nested: id.is_multiple_of(5),
            hidden: false,
            invisible: false,
            icon: id.is_multiple_of(3),
            role: id.is_multiple_of(4),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Flag {
    Background,
    Hover,
    Focusable,
    Component,
    Nested,
    Hidden,
    Invisible,
    Icon,
    Role,
}

#[derive(Clone, Debug)]
enum Change {
    Word {
        item: usize,
        word: usize,
    },
    Color {
        item: usize,
        color: usize,
    },
    Width {
        item: usize,
        width: f32,
    },
    Toggle {
        item: usize,
        flag: Flag,
    },
    Insert {
        at: usize,
    },
    Remove {
        at: usize,
    },
    Move {
        from: usize,
        to: usize,
    },
    Keyed,
    /// Gives the plain section a hover style, so that the rows in it, drawn
    /// again as nested in it until then, are drawn again on their own.
    PlainHover,
    SectionColor {
        color: usize,
    },
    SectionOpacity {
        opacity: f32,
    },
    ClipWidth {
        width: f32,
    },
    Scroll {
        y: f32,
    },
    /// Sets the height of a spacer above the board, moving all of it.
    Spacer {
        height: f32,
    },
    /// Sets the margin left of the clipped section, moving it with its clip.
    Margin {
        left: f32,
    },
    Focus {
        item: usize,
    },
    Blur,
    /// Notifies the view without changing anything, so every element is
    /// built as it was.
    Notify,
    /// Changes nothing and notifies nothing, so the view is drawn again from
    /// last frame, carrying its elements' records along.
    Nothing,
    /// Notifies only the view nested in the board.
    Nested,
    /// Changes the board's quote and the nested view's count, on every step
    /// of a run of steps, so that elements built anew frame after frame
    /// rest, and are drawn again once the run ends.
    Tick,
    MoveMouse {
        x: f32,
        y: f32,
    },
    Resize {
        width: f32,
        height: f32,
    },
}

impl Change {
    fn random(rng: &mut StdRng) -> Self {
        let item = rng.random_range(0..64);
        match rng.random_range(0..100) {
            0..16 => Change::Word {
                item,
                word: rng.random_range(0..WORDS.len()),
            },
            16..22 => Change::Color {
                item,
                color: rng.random_range(0..PALETTE.len()),
            },
            22..27 => Change::Width {
                item,
                width: rng.random_range(20.0..160.0),
            },
            27..37 => Change::Toggle {
                item,
                flag: [
                    Flag::Background,
                    Flag::Hover,
                    Flag::Focusable,
                    Flag::Component,
                    Flag::Nested,
                    Flag::Hidden,
                    Flag::Invisible,
                    Flag::Icon,
                    Flag::Role,
                ][rng.random_range(0..9)],
            },
            37..42 => Change::Insert { at: item },
            42..47 => Change::Remove { at: item },
            47..52 => Change::Move {
                from: item,
                to: rng.random_range(0..64),
            },
            52..54 => Change::Keyed,
            54..55 => Change::PlainHover,
            55..58 => Change::SectionColor {
                color: rng.random_range(0..PALETTE.len()),
            },
            58..61 => Change::SectionOpacity {
                opacity: [1.0, 0.5, 0.8][rng.random_range(0..3)],
            },
            61..64 => Change::ClipWidth {
                width: rng.random_range(40.0..400.0),
            },
            64..66 => Change::Scroll {
                y: rng.random_range(0.0..200.0),
            },
            // By half pixels, whole device pixels at a scale factor of 2.
            66..68 => Change::Scroll {
                y: rng.random_range(0..400) as f32 / 2.,
            },
            68..71 => Change::Focus { item },
            71..72 => Change::Blur,
            72..82 => Change::Notify,
            82..87 => Change::Nothing,
            87..89 => Change::Nested,
            89..90 => {
                if rng.random_bool(0.5) {
                    Change::Spacer {
                        height: rng.random_range(0..60) as f32 / 2.,
                    }
                } else {
                    Change::Margin {
                        left: [0., 0.5, 3., 7.25, 12.][rng.random_range(0..5)],
                    }
                }
            }
            90..97 => Change::MoveMouse {
                x: rng.random_range(0.0..800.0),
                y: rng.random_range(0.0..600.0),
            },
            _ => Change::Resize {
                width: rng.random_range(300.0..900.0),
                height: rng.random_range(240.0..700.0),
            },
        }
    }
}

/// A row of the board drawn by a component, as an application's row
/// components draw theirs.
#[derive(IntoElement)]
struct Card {
    item: Item,
    keyed: bool,
    focus: Option<FocusHandle>,
}

impl RenderOnce for Card {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        render_item(self.item, self.keyed, self.focus.as_ref())
    }
}

fn render_item(item: Item, keyed: bool, focus: Option<&FocusHandle>) -> AnyElement {
    let label = div()
        .text_color(PALETTE[(item.color + 1) % PALETTE.len()])
        .child(SharedString::from(WORDS[item.word]));
    let label = if item.role {
        label
            .id(("label", item.id))
            .role(Role::Label)
            .aria_label(WORDS[item.word])
            .into_any_element()
    } else {
        label.into_any_element()
    };
    let icon = item.icon.then(|| {
        let icon = svg().size(px(8.)).text_color(PALETTE[item.color]);
        match item.word % 3 {
            0 => icon.path(ICONS[item.word / 3 % ICONS.len()]),
            1 => icon.data(ICONS[item.word / 3 % ICONS.len()].as_bytes()),
            _ => icon
                .path(ICONS[0])
                .with_transformation(Transformation::rotate(radians(item.color as f32))),
        }
    });
    let element = div()
        .flex()
        .flex_row()
        .gap_1()
        .w(px(item.width))
        .min_h(px(12.))
        .when(item.background, |this| this.bg(PALETTE[item.color]))
        .when(item.hidden, |this| this.hidden())
        .when(item.invisible, |this| this.invisible())
        .when(item.hover, |this| this.hover(|style| style.bg(PALETTE[4])))
        .when_some(focus.filter(|_| item.focusable), |this, focus| {
            this.track_focus(focus)
                .focus(|style| style.border_1().border_color(PALETTE[3]))
        })
        .children(icon)
        .child(label)
        .child(
            div()
                .size(px(6.))
                .bg(PALETTE[item.id as usize % PALETTE.len()]),
        )
        .when(item.nested, |this| {
            this.child(
                div()
                    .p_1()
                    .child(div().child(WORDS[(item.word + 1) % WORDS.len()]))
                    .child(format!("#{}", item.id)),
            )
        });
    if keyed {
        element.id(("item", item.id)).into_any_element()
    } else {
        element.into_any_element()
    }
}

struct Board {
    items: Vec<Item>,
    next_id: u64,
    keyed: bool,
    plain_hover: bool,
    section_color: usize,
    section_opacity: f32,
    clip_width: f32,
    spacer: f32,
    margin: f32,
    scroll: ScrollHandle,
    focus: Vec<FocusHandle>,
    nested: Entity<Ticker>,
    quote: usize,
}

impl Board {
    fn new(cx: &mut Context<Self>) -> Self {
        Board {
            items: (0..24).map(Item::new).collect(),
            next_id: 24,
            keyed: false,
            plain_hover: false,
            section_color: 0,
            section_opacity: 1.,
            clip_width: 200.,
            spacer: 0.,
            margin: 0.,
            scroll: ScrollHandle::new(),
            focus: (0..128).map(|_| cx.focus_handle()).collect(),
            nested: cx.new(|_| Ticker { count: 0 }),
            quote: 0,
        }
    }

    fn row(&self, item: Item) -> AnyElement {
        let focus = Some(self.focus[item.id as usize % self.focus.len()].clone());
        if item.component {
            Card {
                item,
                keyed: self.keyed,
                focus,
            }
            .into_any_element()
        } else {
            render_item(item, self.keyed, focus.as_ref())
        }
    }

    fn apply(&mut self, change: &Change, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.items.len();
        let item = |at: usize| if len == 0 { None } else { Some(at % len) };
        match *change {
            Change::Word { item: at, word } => {
                if let Some(at) = item(at) {
                    self.items[at].word = word;
                }
            }
            Change::Color { item: at, color } => {
                if let Some(at) = item(at) {
                    self.items[at].color = color;
                }
            }
            Change::Width { item: at, width } => {
                if let Some(at) = item(at) {
                    self.items[at].width = width;
                }
            }
            Change::Toggle { item: at, flag } => {
                if let Some(at) = item(at) {
                    let item = &mut self.items[at];
                    let value = match flag {
                        Flag::Background => &mut item.background,
                        Flag::Hover => &mut item.hover,
                        Flag::Focusable => &mut item.focusable,
                        Flag::Component => &mut item.component,
                        Flag::Nested => &mut item.nested,
                        Flag::Hidden => &mut item.hidden,
                        Flag::Invisible => &mut item.invisible,
                        Flag::Icon => &mut item.icon,
                        Flag::Role => &mut item.role,
                    };
                    *value = !*value;
                }
            }
            Change::Insert { at } => {
                let at = at % (len + 1);
                self.items.insert(at, Item::new(self.next_id));
                self.next_id += 1;
            }
            Change::Remove { at } => {
                if let Some(at) = item(at) {
                    self.items.remove(at);
                }
            }
            Change::Move { from, to } => {
                if let Some(from) = item(from) {
                    let moved = self.items.remove(from);
                    let to = to % (self.items.len() + 1);
                    self.items.insert(to, moved);
                }
            }
            Change::Keyed => self.keyed = !self.keyed,
            Change::PlainHover => self.plain_hover = !self.plain_hover,
            Change::SectionColor { color } => self.section_color = color,
            Change::SectionOpacity { opacity } => self.section_opacity = opacity,
            Change::ClipWidth { width } => self.clip_width = width,
            Change::Scroll { y } => self.scroll.set_offset(point(px(0.), px(-y))),
            Change::Spacer { height } => self.spacer = height,
            Change::Margin { left } => self.margin = left,
            Change::Focus { item: at } => {
                if let Some(at) = item(at) {
                    let id = self.items[at].id as usize;
                    window.focus(&self.focus[id % self.focus.len()], cx);
                }
            }
            Change::Blur => window.blur(cx),
            Change::Notify => {}
            Change::Tick => self.quote += 1,
            Change::Nothing | Change::Nested | Change::MoveMouse { .. } | Change::Resize { .. } => {
                return;
            }
        }
        cx.notify();
    }
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let third = self.items.len() / 3;
        let (first, rest) = self.items.split_at(third);
        let (second, third) = rest.split_at(rest.len() / 2);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().h(px(self.spacer)))
            // Inherits a text color and an opacity that change.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .text_color(PALETTE[self.section_color])
                    .opacity(self.section_opacity)
                    .children(first.iter().map(|&item| self.row(item))),
            )
            // Clipped to a width that changes, beside a margin that changes.
            .child(
                div()
                    .ml(px(self.margin))
                    .overflow_hidden()
                    .w(px(self.clip_width))
                    .h(px(48.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .children(second.iter().map(|&item| self.row(item))),
                    ),
            )
            // Scrolled.
            .child(
                div()
                    .id("scrolled")
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .h(px(80.))
                    .w(px(260.))
                    .children(third.iter().map(|&item| self.row(item))),
            )
            // Deferred, with a view nested in it.
            .child(
                div().size(px(10.)).child(deferred(
                    anchored().child(
                        div()
                            .flex()
                            .flex_col()
                            .bg(PALETTE[1])
                            .children(first.iter().take(3).map(|&item| self.row(item)))
                            .child(self.nested.clone()),
                    ),
                )),
            )
            // Every item as a plain row, keyed or not, so that the section
            // and its rows can be drawn again, and rows moving among their
            // siblings are found where they were.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when(self.plain_hover, |this| {
                        this.hover(|style| style.bg(PALETTE[3]))
                    })
                    .children(self.items.iter().map(|&item| {
                        render_item(
                            Item {
                                hover: false,
                                focusable: false,
                                ..item
                            },
                            self.keyed,
                            None,
                        )
                    })),
            )
            // Changes on every step of a run.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .child(div().w(px(40.)).child("quote"))
                    .child(
                        div()
                            .w(px(40.))
                            .child(SharedString::from(self.quote.to_string())),
                    )
                    .child(div().size(px(8.)).bg(PALETTE[self.quote % PALETTE.len()])),
            )
            // Clipped by a box larger than the window, which the spacer
            // moves: what it holds is clipped as by the window until the
            // box's top edge comes into it.
            .child(
                div()
                    .absolute()
                    .left(px(-600.))
                    .top(px(-600. + self.spacer * 40.))
                    .size(px(3000.))
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(px(650.))
                            .top(px(650.))
                            .child("curtain"),
                    ),
            )
            // Never changes.
            .child(
                div().flex().flex_row().gap_2().children(
                    WORDS
                        .iter()
                        .map(|&word| div().px_1().bg(PALETTE[2]).child(word)),
                ),
            )
    }
}

/// A view nested in the board, notified on its own.
struct Ticker {
    count: usize,
}

impl Render for Ticker {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(px(10.))
                    .h(px(6.))
                    .bg(PALETTE[self.count % PALETTE.len()]),
            )
            .child(SharedString::from(self.count.to_string()))
    }
}

/// The no-op text system, except that every glyph rasterizes to a small box,
/// so text paints a sprite per glyph and where each glyph went is compared.
pub(super) struct GlyphBoxTextSystem(pub(super) NoopTextSystem);

impl PlatformTextSystem for GlyphBoxTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.0.add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        self.0.all_font_names()
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        self.0.font_id(descriptor)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.0.font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        self.0.typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.0.advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.0.glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        Ok(Bounds {
            origin: point(DevicePixels(0), DevicePixels(-8)),
            size: size(DevicePixels(5), DevicePixels(9)),
        })
    }

    fn rasterize_glyph(
        &self,
        _params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        let area = raster_bounds.size.width.0 * raster_bounds.size.height.0;
        Ok((raster_bounds.size, vec![u8::MAX; area as usize]))
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.0.layout_line(text, font_size, runs)
    }

    fn recommended_rendering_mode(&self, font_id: FontId, font_size: Pixels) -> TextRenderingMode {
        self.0.recommended_rendering_mode(font_id, font_size)
    }
}

fn apply(cx: &mut TestAppContext, window: WindowHandle<Board>, change: &Change) {
    match *change {
        Change::MoveMouse { x, y } => {
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(x), px(y)),
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        }
        Change::Resize { width, height } => {
            cx.simulate_window_resize(window.into(), size(px(width), px(height)));
        }
        Change::Nested | Change::Tick => {
            let nested = window
                .read_with(cx, |board, _| board.nested.clone())
                .unwrap();
            // In one update, so that the board is not drawn between them.
            cx.update(|cx| {
                nested.update(cx, |ticker, cx| {
                    ticker.count += 1;
                    cx.notify();
                });
                if let Change::Tick = change {
                    window
                        .update(cx, |board, window, cx| board.apply(change, window, cx))
                        .unwrap();
                }
            });
        }
        _ => window
            .update(cx, |board, window, cx| board.apply(change, window, cx))
            .unwrap(),
    }
}

/// What a window's last frame left besides what it painted: its dispatch
/// tree, which node is focused, and the listeners and handlers it holds.
fn describe_frame_state(window: &Window) -> Vec<String> {
    let frame = &window.rendered_frame;
    let focused = window
        .focus
        .and_then(|focus| frame.dispatch_tree.focusable_node_ids.get(&focus).copied());
    let mut lines: Vec<String> = frame
        .dispatch_tree
        .nodes
        .iter()
        .enumerate()
        .map(|(ix, node)| {
            format!(
                "node {ix} parent {:?} context {:?} focusable {} view {} keys {} actions {} modifiers {}",
                node.parent.map(|parent| parent.0),
                node.context.as_ref().map(|context| format!("{context:?}")),
                node.focus_id.is_some(),
                node.view_id.is_some(),
                node.key_listeners.len(),
                node.action_listeners.len(),
                node.modifiers_changed_listeners.len(),
            )
        })
        .collect();
    lines.push(format!("focused node {:?}", focused.map(|node| node.0)));
    lines.push(format!("mouse listeners {}", frame.mouse_listeners.len()));
    lines.push(format!(
        "input handlers {}",
        frame.input_handlers.iter().filter(|h| h.is_some()).count()
    ));
    lines.push(format!("cursor styles {}", frame.cursor_styles.len()));
    // Which glyph or icon each sprite shows, and how it is turned: the two
    // windows share one atlas, so the same image is the same tile in both.
    lines.extend(frame.scene.monochrome_sprites.iter().map(|sprite| {
        format!(
            "sprite {:?} tile {:?} {:?}",
            sprite.bounds, sprite.tile.tile_id, sprite.transformation
        )
    }));
    lines
}

/// Draws a frame, after forgetting what the window retains if asked, and
/// returns what it drew and how many elements it drew again.
fn draw(
    cx: &mut TestAppContext,
    window: WindowHandle<Board>,
    from_scratch: bool,
) -> (Vec<String>, Vec<String>, u64, u64) {
    cx.update_window(window.into(), |_, window, cx| {
        // The incremental window counts the elements drawn again since its
        // last frame was compared, which the test app draws too when a
        // change notifies it.
        if from_scratch {
            window.forget_retained_state();
            window.reset_layout_stats();
        }
        window.draw(cx).clear(cx);
        let stats = window.layout_stats();
        window.reset_layout_stats();
        (
            window.describe_rendered_frame(),
            describe_frame_state(window),
            stats.elements_reused,
            stats.elements_moved,
        )
    })
    .unwrap()
}

fn first_difference(actual: &[String], expected: &[String]) -> Option<(usize, String)> {
    if actual == expected {
        return None;
    }
    let first = actual
        .iter()
        .zip(expected)
        .position(|(actual, expected)| actual != expected)
        .unwrap_or(actual.len().min(expected.len()));
    let excerpt = |lines: &[String]| {
        lines
            .iter()
            .enumerate()
            .skip(first.saturating_sub(2))
            .take(5)
            .map(|(ix, line)| format!("  {ix}: {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some((
        first,
        format!(
            "({} lines against {})\nincremental:\n{}\nfrom scratch:\n{}",
            actual.len(),
            expected.len(),
            excerpt(actual),
            excerpt(expected)
        ),
    ))
}

/// Drives both windows through one random history and returns how many
/// elements the incremental window drew again along the way, and how many
/// of those moved.
fn run(seed: u64, steps: usize) -> (u64, u64) {
    let mut cx = TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)));
    cx.update(|cx| cx.svg_renderer = SvgRenderer::new(Arc::new(Icons)));
    let incremental = cx.add_window(|_, cx| Board::new(cx));
    let from_scratch = cx.add_window(|_, cx| Board::new(cx));
    let atlas = cx
        .update_window(incremental.into(), |_, window, _| {
            window.sprite_atlas.clone()
        })
        .unwrap();
    cx.update_window(from_scratch.into(), |_, window, _| {
        window.sprite_atlas = atlas;
    })
    .unwrap();
    let mut rng = StdRng::seed_from_u64(seed);
    let mut history: Vec<Vec<Change>> = Vec::new();
    let mut reused = 0;
    let mut moved = 0;
    let mut ticking = false;

    for step in 0..steps {
        let mut changes: Vec<Change> = if step == 0 {
            Vec::new()
        } else {
            (0..rng.random_range(1..=3))
                .map(|_| Change::random(&mut rng))
                .collect()
        };
        if rng.random_ratio(1, 6) {
            ticking = !ticking;
        }
        if ticking {
            changes.push(Change::Tick);
        }
        for change in &changes {
            apply(&mut cx, incremental, change);
            apply(&mut cx, from_scratch, change);
        }
        history.push(changes);

        let (expected, expected_state, reused_from_scratch, _) = draw(&mut cx, from_scratch, true);
        let (actual, actual_state, reused_incrementally, moved_incrementally) =
            draw(&mut cx, incremental, false);
        assert_eq!(
            reused_from_scratch, 0,
            "a window drawing from scratch cannot draw an element again"
        );
        reused += reused_incrementally;
        moved += moved_incrementally;

        for (what, actual, expected) in [
            ("painted frame", &actual, &expected),
            ("frame state", &actual_state, &expected_state),
        ] {
            if let Some((line, difference)) = first_difference(actual, expected) {
                let history = history
                    .iter()
                    .enumerate()
                    .map(|(step, changes)| format!("  {step}: {changes:?}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                panic!(
                    "seed {seed}, step {step}: the incremental {what} differs from the one \
                     drawn from scratch at line {line} {difference}\nchanges so far:\n{history}"
                );
            }
        }
    }
    (reused, moved)
}

#[test]
fn frames_drawing_elements_again_match_frames_drawn_from_scratch() {
    let (reused, moved) = (0..32)
        .map(|seed| run(seed, 60))
        .fold((0, 0), |(reused, moved), run| {
            (reused + run.0, moved + run.1)
        });
    assert!(
        reused > 0,
        "the incremental window never drew an element again, so nothing was compared"
    );
    assert!(
        moved > 0,
        "the incremental window never drew an element again moved, so no move was compared"
    );
}
