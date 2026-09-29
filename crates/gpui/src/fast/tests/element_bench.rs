//! How much drawing a quote board costs when its view renders every frame
//! and only a few of its rows change, with and without retained elements.
//! Ignored by default; run it with
//!
//! ```text
//! cargo test -p gpui --lib --release element_bench -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use crate::{
    App, Context, IntoElement, LayoutStats, Render, RenderOnce, SharedString, TestAppContext,
    Window, WindowHandle, div, hsla, prelude::*, px,
};

#[derive(Clone)]
struct Quote {
    symbol: SharedString,
    name: SharedString,
    price: f64,
    change: f64,
    volume: u64,
    history: [u8; 8],
}

/// One row of the board, built by the board's render every frame.
#[derive(IntoElement)]
struct TradingRow {
    quote: Quote,
}

impl RenderOnce for TradingRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let quote = self.quote;
        let color = if quote.change >= 0. {
            hsla(0.33, 0.6, 0.45, 1.)
        } else {
            hsla(0.0, 0.6, 0.5, 1.)
        };
        div()
            .flex()
            .flex_row()
            .h(px(20.))
            .gap_1()
            .child(div().w(px(60.)).child(quote.symbol))
            .child(div().w(px(120.)).child(quote.name))
            .child(
                div()
                    .w(px(60.))
                    .text_color(color)
                    .child(SharedString::from(format!("{:.2}", quote.price))),
            )
            .child(
                div()
                    .w(px(60.))
                    .text_color(color)
                    .child(SharedString::from(format!("{:+.2}%", quote.change))),
            )
            .child(
                div()
                    .w(px(80.))
                    .child(SharedString::from(quote.volume.to_string())),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .w(px(40.))
                    .h_full()
                    .children(quote.history.iter().map(|&height| {
                        div()
                            .w(px(4.))
                            .h(px(height as f32))
                            .bg(hsla(0.6, 0.5, 0.5, 1.))
                    })),
            )
            .child(div().size(px(12.)).rounded_full().bg(color))
            .child(
                div()
                    .px_1()
                    .rounded_sm()
                    .bg(hsla(0.6, 0.2, 0.3, 1.))
                    .child("Trade"),
            )
    }
}

struct Board {
    quotes: Vec<Quote>,
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(0.6, 0.1, 0.1, 1.))
            .children(self.quotes.iter().map(|quote| TradingRow {
                quote: quote.clone(),
            }))
    }
}

fn quotes(rows: usize) -> Vec<Quote> {
    (0..rows)
        .map(|row| Quote {
            symbol: format!("SYM{row}").into(),
            name: format!("Company number {row}").into(),
            price: 100. + row as f64,
            change: (row % 7) as f64 - 3.,
            volume: 1_000_000 + row as u64 * 37,
            history: std::array::from_fn(|i| ((row + i * 3) % 16) as u8),
        })
        .collect()
}

/// What drawing the board cost per frame.
pub(crate) struct BoardCost {
    pub(crate) frame: Duration,
    pub(crate) stats: LayoutStats,
}

/// Draws `frames` frames of a board of `rows` rows, `changing` of whose
/// prices change before each, the board notified every frame.
pub(crate) fn measure_board(
    element_retention: bool,
    rows: usize,
    changing: usize,
    frames: usize,
) -> BoardCost {
    let mut cx = TestAppContext::single();
    let window: WindowHandle<Board> = cx.add_window(|_, _| Board {
        quotes: quotes(rows),
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.set_element_retention(element_retention);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    // Rendered again until its rows are recorded, which they are once they
    // stand still.
    for _ in 0..3 {
        window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    }
    cx.update_window(window.into(), |_, window, _| window.reset_layout_stats())
        .unwrap();

    let started = Instant::now();
    for frame in 0..frames {
        window
            .update(&mut cx, |board, _, cx| {
                for n in 0..changing {
                    let row = (frame * 7 + n * 13) % board.quotes.len();
                    board.quotes[row].price += 0.01;
                }
                cx.notify();
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }
    let frame = started.elapsed() / frames as u32;
    let stats = cx
        .update_window(window.into(), |_, window, _| window.layout_stats())
        .unwrap();
    BoardCost { frame, stats }
}

#[test]
#[ignore]
fn element_bench() {
    let frames = 300;
    for (rows, changing) in [(100, 0), (100, 1), (100, 10), (100, 100)] {
        let off = measure_board(false, rows, changing, frames);
        let on = measure_board(true, rows, changing, frames);
        let per_frame = |d: Duration| d.as_secs_f64() * 1e3 / frames as f64;
        println!(
            "{rows} rows, {changing:>3} changed per frame: off {:>7.3} ms, on {:>7.3} ms ({:+.0}%)",
            off.frame.as_secs_f64() * 1e3,
            on.frame.as_secs_f64() * 1e3,
            (on.frame.as_secs_f64() / off.frame.as_secs_f64() - 1.) * 100.,
        );
        for (name, cost) in [("off", &off), ("on ", &on)] {
            let stats = &cost.stats;
            println!(
                "    {name}: build {:.3} prepaint {:.3} paint {:.3} ms/frame, elements built {:.0} reused {:.0} /frame",
                per_frame(stats.build_time),
                per_frame(stats.prepaint_time),
                per_frame(stats.paint_time),
                stats.elements_built as f64 / frames as f64,
                stats.elements_reused as f64 / frames as f64,
            );
        }
    }
}

/// The board with element retention on, drawn long enough to be sampled by
/// a profiler.
#[test]
#[ignore]
fn element_bench_profile() {
    let rows = std::env::var("BOARD_ROWS").map_or(100, |rows| rows.parse().unwrap());
    let changing = std::env::var("BOARD_CHANGING").map_or(10, |n| n.parse().unwrap());
    let retention = std::env::var("BOARD_RETENTION").map_or(true, |on| on != "0");
    let cost = measure_board(retention, rows, changing, 20_000);
    println!("{:.3} ms/frame", cost.frame.as_secs_f64() * 1e3);
}
