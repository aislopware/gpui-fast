//! A strip of terminal tiles, built the way Slopty builds its main window: a
//! remote-work app scrolling a row of tiles sideways, niri-style.
//!
//! - The root view holds a title bar and a status bar, each a cached view of
//!   its own, and the strip between them.
//! - The strip is one view drawing every tile's frame and header inline: a
//!   header of a shell icon, the title and directory, a readout of how long
//!   the running command has run, and a row of icon buttons that highlight
//!   under the pointer, all with listeners, accessibility roles and hover
//!   styles, as Slopty's are.
//! - Each tile's body is a terminal: a view of its own, drawn cached, whose
//!   root holds the keyboard focus and listeners, and whose grid is one
//!   custom element that shapes each row and paints cell backgrounds and
//!   glyphs, as a terminal renderer does.
//!
//! The scenarios differ in what changes each frame: one terminal printing,
//! the strip's readout ticking, the strip scrolling, the pointer moving over
//! the tile headers, the keyboard focus moving between terminals, or only
//! the status bar's clock.

use std::{ops::Range, rc::Rc};

use gpui::{
    AnyView, App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, Hsla, InputEvent as _, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseMoveEvent, Pixels, Render, Role, ShapedLine,
    SharedString, StyleRefinement, TextRun, UTF16Selection, Window, div, fill, font, hsla, point,
    prelude::*, px, relative, size, svg,
};

pub fn scenarios() -> Vec<Box<dyn crate::Scenario>> {
    vec![
        Box::new(StripScenario {
            name: "strip-output",
            description: "Eight terminal tiles in a strip; each frame one terminal prints a line, notifying only its own view.",
            kind: Kind::Output,
        }),
        Box::new(StripScenario {
            name: "strip-readout",
            description: "Eight terminal tiles; each frame the strip is notified as a running command's readout ticks, building every tile header again.",
            kind: Kind::Readout,
        }),
        Box::new(StripScenario {
            name: "strip-scroll",
            description: "Eight terminal tiles; the strip scrolls sideways 6.5px a frame, moving every tile and its terminal.",
            kind: Kind::Scroll,
        }),
        Box::new(StripScenario {
            name: "strip-spring",
            description: "strip-scroll with a terminal holding the keyboard: each grid writes its view as it is prepainted and registers an input handler as it is painted, as Slopty's does.",
            kind: Kind::Spring,
        }),
        Box::new(StripScenario {
            name: "strip-hover",
            description: "Eight terminal tiles; the pointer sweeps across the tile headers and their icon buttons, one move a frame.",
            kind: Kind::Hover,
        }),
        Box::new(StripScenario {
            name: "strip-focus",
            description: "Eight terminal tiles, each ringed while it holds the keyboard; every ten frames the focus moves to the next terminal, as picking a tile does.",
            kind: Kind::Focus,
        }),
        Box::new(StripScenario {
            name: "strip-clock",
            description: "Eight terminal tiles at rest; only the status bar's clock changes each frame.",
            kind: Kind::Clock,
        }),
    ]
}

const TILES: usize = 8;
const TILE_WIDTH: f32 = 560.;
const TILE_GAP: f32 = 12.;
const HEADER_HEIGHT: f32 = 30.;
const ROWS: usize = 40;
const COLUMNS: usize = 90;
const LINE_HEIGHT: f32 = 18.;
const FONT_SIZE: f32 = 13.;

const BG: Hsla = hsla(0.62, 0.12, 0.09, 1.);
const SURFACE: Hsla = hsla(0.62, 0.1, 0.13, 1.);
const HOVER: Hsla = hsla(0.62, 0.1, 0.2, 1.);
const PRESSED: Hsla = hsla(0.62, 0.1, 0.25, 1.);
const FG: Hsla = hsla(0., 0., 0.88, 1.);
const MUTED: Hsla = hsla(0., 0., 0.55, 1.);
const ACCENT: Hsla = hsla(0.58, 0.8, 0.6, 1.);
const GREEN: Hsla = hsla(0.33, 0.6, 0.5, 1.);
const SELECTION: Hsla = hsla(0.58, 0.5, 0.3, 1.);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Output,
    Readout,
    Scroll,
    Spring,
    Hover,
    Focus,
    Clock,
}

struct StripScenario {
    name: &'static str,
    description: &'static str,
    kind: Kind,
}

impl crate::Scenario for StripScenario {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        let spring = self.kind == Kind::Spring;
        let ring = self.kind == Kind::Focus;
        let shell = cx.new(|cx| Shell::new(spring, ring, cx));
        if spring || ring {
            let focus = shell.read(cx).strip.read(cx).tiles[0]
                .terminal
                .read(cx)
                .focus
                .clone();
            window.focus(&focus, cx);
        }
        shell.into()
    }

    fn step(&self, root: &AnyView, frame: usize, window: &mut Window, cx: &mut App) {
        let shell: Entity<Shell> = root.clone().downcast().unwrap();
        let (strip, status) = {
            let shell = shell.read(cx);
            (shell.strip.clone(), shell.status.clone())
        };
        match self.kind {
            Kind::Output => {
                let terminal = strip.read(cx).tiles[frame % TILES].terminal.clone();
                terminal.update(cx, |terminal, cx| {
                    terminal.print(frame);
                    cx.notify();
                });
            }
            Kind::Readout => strip.update(cx, |strip, cx| {
                strip.tiles[0].running = Some(frame as u64);
                cx.notify();
            }),
            Kind::Scroll | Kind::Spring => strip.update(cx, |strip, cx| {
                let span = (TILE_WIDTH + TILE_GAP) * TILES as f32;
                strip.scroll = (frame as f32 * 6.5) % (span / 2.);
                cx.notify();
            }),
            Kind::Hover => {
                // Along the header row, across two tiles and their buttons.
                let x = 8. + (frame % 140) as f32 * 8.;
                let y = 30. + HEADER_HEIGHT / 2.;
                window.dispatch_event(
                    MouseMoveEvent {
                        position: point(px(x), px(y)),
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            }
            Kind::Focus => {
                if frame.is_multiple_of(10) {
                    let tile = &strip.read(cx).tiles[frame / 10 % TILES];
                    let focus = tile.terminal.read(cx).focus.clone();
                    window.focus(&focus, cx);
                }
            }
            Kind::Clock => status.update(cx, |status, cx| {
                status.seconds = frame as u64;
                cx.notify();
            }),
        }
    }
}

struct Shell {
    title: Entity<TitleBar>,
    strip: Entity<Strip>,
    status: Entity<StatusBar>,
}

impl Shell {
    fn new(spring: bool, ring: bool, cx: &mut Context<Self>) -> Self {
        let tiles = (0..TILES)
            .map(|index| Tile {
                title: format!("shell {index}").into(),
                cwd: format!("~/src/project-{index}/crates").into(),
                running: (index % 3 == 0).then_some(12),
                terminal: cx.new(|cx| Terminal::new(index, spring, ring, cx)),
            })
            .collect();
        Shell {
            title: cx.new(|_| TitleBar),
            strip: cx.new(|_| Strip { tiles, scroll: 0. }),
            status: cx.new(|_| StatusBar { seconds: 0 }),
        }
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bar = || StyleRefinement::default().w_full().h(px(HEADER_HEIGHT));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(BG)
            .text_color(FG)
            .text_size(px(FONT_SIZE))
            .child(self.title.clone().cached(bar()))
            .child(div().flex_1().relative().child(self.strip.clone()))
            .child(self.status.clone().cached(bar()))
    }
}

struct TitleBar;

impl Render for TitleBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .bg(SURFACE)
            .children(["Workspace", "Agents", "Remote"].map(|tab| {
                div()
                    .id(tab)
                    .role(Role::Tab)
                    .px_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|style| style.bg(HOVER))
                    .on_click(|_, _, _| {})
                    .child(tab)
            }))
    }
}

struct StatusBar {
    seconds: u64,
}

impl Render for StatusBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clock: SharedString = format!(
            "{:02}:{:02}:{:02}",
            self.seconds / 3600 % 24,
            self.seconds / 60 % 60,
            self.seconds % 60
        )
        .into();
        let readout = |label: &'static str, value: SharedString| {
            div()
                .flex()
                .gap_1()
                .child(div().text_color(MUTED).child(label))
                .child(value)
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .bg(SURFACE)
            .text_color(MUTED)
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(readout("workers", "3 up".into()))
                    .child(div().size(px(4.)).rounded_full().bg(MUTED))
                    .child(readout("latency", "2.1 ms".into()))
                    .child(div().size(px(4.)).rounded_full().bg(MUTED))
                    .child(readout("stream", "120 fps".into())),
            )
            .child(div().id("clock").role(Role::Timer).child(clock))
    }
}

struct Tile {
    title: SharedString,
    cwd: SharedString,
    running: Option<u64>,
    terminal: Entity<Terminal>,
}

struct Strip {
    tiles: Vec<Tile>,
    scroll: f32,
}

impl Render for Strip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().overflow_hidden().children(
            self.tiles
                .iter()
                .enumerate()
                .map(|(index, tile)| render_tile(index, tile, self.scroll, cx)),
        )
    }
}

fn render_tile(index: usize, tile: &Tile, scroll: f32, cx: &Context<Strip>) -> impl IntoElement {
    let left = TILE_GAP + index as f32 * (TILE_WIDTH + TILE_GAP) - scroll;
    div()
        .id(("tile", index))
        .group("tile")
        .role(Role::Group)
        .aria_label(tile.title.clone())
        .absolute()
        .left(px(left))
        .top(px(TILE_GAP))
        .bottom(px(TILE_GAP))
        .w(px(TILE_WIDTH))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded_md()
        .bg(BG)
        .border_1()
        .border_color(SURFACE)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |strip, _, _, cx| {
                strip.tiles[index].running = None;
                cx.notify();
            }),
        )
        .child(render_header(index, tile, cx))
        .child(
            tile.terminal
                .clone()
                .cached(StyleRefinement::default().flex_1().w_full()),
        )
}

fn render_header(index: usize, tile: &Tile, cx: &Context<Strip>) -> impl IntoElement {
    let readout = tile.running.map(|seconds| {
        div()
            .id(("readout", index))
            .role(Role::Status)
            .flex()
            .items_center()
            .gap_1()
            .text_color(GREEN)
            .child(icon("icons/play.svg", GREEN))
            .child(SharedString::from(format!(
                "{}:{:02}",
                seconds / 60,
                seconds % 60
            )))
    });
    div()
        .id(("header", index))
        .role(Role::Heading)
        .h(px(HEADER_HEIGHT))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .bg(SURFACE)
        .cursor_grab()
        .on_mouse_down(MouseButton::Left, |_, _, _| {})
        .child(icon("icons/terminal.svg", ACCENT))
        .child(
            div()
                .id(("title", index))
                .role(Role::Label)
                .child(tile.title.clone()),
        )
        .child(div().text_color(MUTED).child(tile.cwd.clone()))
        .children(readout)
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap_1()
                .group_hover("tile", |style| style.opacity(1.))
                .opacity(0.6)
                .children(
                    ["icons/split.svg", "icons/zoom.svg", "icons/close.svg"]
                        .into_iter()
                        .enumerate()
                        .map(|(button, path)| icon_button(index * 8 + button, path, cx)),
                ),
        )
}

fn icon(path: &'static str, color: Hsla) -> impl IntoElement {
    svg().path(path).size(px(14.)).flex_none().text_color(color)
}

fn icon_button(id: usize, path: &'static str, cx: &Context<Strip>) -> impl IntoElement {
    div()
        .id(("button", id))
        .role(Role::Button)
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(|style| style.bg(HOVER))
        .active(|style| style.bg(PRESSED))
        .on_click(cx.listener(|_, _, _, cx| cx.notify()))
        .child(icon(path, MUTED))
}

struct Terminal {
    lines: Vec<SharedString>,
    focus: FocusHandle,
    printed: usize,
    /// Whether its grid writes it as it is prepainted and registers it as
    /// the input handler as it is painted.
    spring: bool,
    /// What its grid wrote last, as Slopty's keeps its shaped rows.
    prepainted: usize,
    /// Whether it is ringed while it holds the keyboard.
    ring: bool,
}

impl Terminal {
    fn new(index: usize, spring: bool, ring: bool, cx: &mut Context<Self>) -> Self {
        Terminal {
            lines: (0..ROWS).map(|row| line(index * 1000 + row)).collect(),
            focus: cx.focus_handle(),
            printed: 0,
            spring,
            prepainted: 0,
            ring,
        }
    }

    fn print(&mut self, seed: usize) {
        self.lines.remove(0);
        self.lines.push(line(seed * 7 + self.printed));
        self.printed += 1;
    }
}

/// A line of shell output: a prompt, a path and some words, padded to width.
fn line(seed: usize) -> SharedString {
    const WORDS: [&str; 10] = [
        "cargo",
        "build",
        "--release",
        "Compiling",
        "gpui",
        "v0.2.2",
        "warning:",
        "unused",
        "Finished",
        "target/release",
    ];
    let mut text = format!("{:>5} $ ", seed % 10_000);
    let mut word = seed;
    while text.len() < COLUMNS - 12 {
        text.push_str(WORDS[word % WORDS.len()]);
        text.push(' ');
        word = word.wrapping_mul(31).wrapping_add(7);
    }
    text.truncate(COLUMNS);
    text.into()
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(BG)
            .p_1()
            .when(self.ring, |this| {
                this.border_1()
                    .border_color(BG)
                    .focus(|style| style.border_color(ACCENT))
            })
            .on_key_down(cx.listener(|_, _, _, _| {}))
            .on_scroll_wheel(cx.listener(|_, _, _, _| {}))
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, _| {}))
            .on_mouse_move(cx.listener(|_, _, _, _| {}))
            .child(Grid {
                lines: Rc::from(self.lines.as_slice()),
                view: self.spring.then(|| (cx.entity(), self.focus.clone())),
            })
    }
}

/// A terminal's grid: one element that shapes and paints every row.
struct Grid {
    lines: Rc<[SharedString]>,
    view: Option<(Entity<Terminal>, FocusHandle)>,
}

impl IntoElement for Grid {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Grid {
    type RequestLayoutState = ();
    type PrepaintState = Vec<ShapedLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<ShapedLine> {
        if let Some((view, _)) = &self.view {
            view.update(cx, |terminal, _| terminal.prepainted += 1);
        }
        let font = font("Lilex");
        self.lines
            .iter()
            .map(|text| {
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: FG,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(text.clone(), px(FONT_SIZE), &[run], None)
            })
            .collect()
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        lines: &mut Vec<ShapedLine>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((view, focus)) = &self.view {
            window.handle_input(focus, ElementInputHandler::new(bounds, view.clone()), cx);
        }
        let cell = px(FONT_SIZE * 0.6);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (row, line) in lines.iter().enumerate() {
                let origin = bounds.origin + point(px(0.), px(row as f32 * LINE_HEIGHT));
                if row % 4 == 1 {
                    // A selection, and a highlighted prompt.
                    window.paint_quad(fill(
                        Bounds::new(
                            origin + point(cell * 8., px(0.)),
                            size(cell * 24., px(LINE_HEIGHT)),
                        ),
                        SELECTION,
                    ));
                    window.paint_quad(fill(
                        Bounds::new(origin, size(cell * 6., px(LINE_HEIGHT))),
                        SURFACE,
                    ));
                }
                let _ = line.paint(
                    origin,
                    px(LINE_HEIGHT),
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        });
    }
}

impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(
            element_bounds.origin,
            size(px(2.), px(LINE_HEIGHT)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
