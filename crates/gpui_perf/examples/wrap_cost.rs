//! What wrapping text costs: a transcript of about two thousand lines (prose,
//! paths, URLs, code, CJK, emoji, Vietnamese) wrapped at three widths, with
//! the platform's text system.
//!
//! - `wrap_line`: [`gpui::LineWrapper::wrap_line`], the path GPUI Kit's editor
//!   wraps its lines through; then over the same lines made of the ASCII words
//!   only, and over those at a width every line fits.
//! - `shape_text`: `WindowTextSystem::shape_text` with a wrap width, as a text
//!   element lays out a paragraph, over the first [`SHAPED_LINES`] lines, few
//!   enough for the recent-shapes cache to keep them all shaped: each round
//!   costs the wrap boundaries and the cache lookups.
//!
//! ```text
//! cargo run -p gpui_perf --example wrap_cost --release
//! ```

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

use gpui::{
    AppContext as _, Context, HeadlessAppContext, IndentAdjustment, IntoElement, LineFragment,
    Render, SharedString, TextRun, Window, black, div, font, px, size,
};
use std::time::{Duration, Instant};

const LINES: usize = 2000;
const WIDTHS: [f32; 3] = [320., 560., 900.];
const ROUNDS: usize = 40;
const SHAPED_LINES: usize = 100;
/// Wider than any line of the fixture.
const FITTING_WIDTH: f32 = 4000.;

const WORDS: &[&str] = &[
    "the",
    "terminal",
    "renders",
    "every",
    "frame",
    "while",
    "Claude",
    "edits",
    "crates/slopty-ui/src/terminal/element.rs:1862",
    "https://github.com/zed-industries/zed/pull/64624#issuecomment-5885169900",
    "non-trivial",
    "fn wrap_line(&mut self, fragments: &[LineFragment]) -> impl Iterator<Item = Boundary>",
    "(see",
    "above),",
    "\"quoted\"",
    "well…",
    "100%",
    "a=1&b=2",
    "你好世界，这是一段中文。",
    "こんにちは",
    "👍🏽",
    "🇯🇵",
    "👩‍💻",
    "Thậm",
    "chí",
    "đến",
    "khi",
    "thua",
    "chạy",
    "/Users/someone/Library/Application Support/Slopty/sessions/2026-09-29/transcript.jsonl",
    "x",
    "—",
];

/// Deterministic lines from `words`, 40 to 300 bytes long.
fn fixture(words: &[&str]) -> Vec<SharedString> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..LINES)
        .map(|_| {
            let target = 40 + (next() % 260) as usize;
            let mut line = String::new();
            while line.len() < target {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(words[(next() % words.len() as u64) as usize]);
            }
            line.into()
        })
        .collect()
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The fastest and the median round, in milliseconds.
fn summary(mut samples: Vec<Duration>) -> String {
    samples.sort();
    let ms = |duration: Duration| duration.as_secs_f64() * 1e3;
    format!(
        "{:>8.3} ms a round at best, {:>8.3} median of {ROUNDS}",
        ms(samples[0]),
        ms(samples[samples.len() / 2])
    )
}

fn main() {
    let text_system = gpui_platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::new(text_system);
    let lines = fixture(WORDS);
    let ascii_words: Vec<&str> = WORDS
        .iter()
        .copied()
        .filter(|word| word.is_ascii())
        .collect();
    let ascii_lines = fixture(&ascii_words);
    let bytes: usize = lines.iter().map(|line| line.len()).sum();
    let ascii = lines.iter().filter(|line| line.is_ascii()).count();
    println!(
        "{} lines, {} KiB, {ascii} of them ASCII, wrapped at {WIDTHS:?} px",
        lines.len(),
        bytes / 1024
    );

    let text_font = font(".SystemUIFont");
    let font_size = px(14.);
    cx.update(|cx| {
        let mut wrapper = cx.text_system().line_wrapper(text_font.clone(), font_size);
        for (name, lines, widths) in [
            ("wrap_line:", &lines, &WIDTHS[..]),
            ("  ASCII:", &ascii_lines, &WIDTHS[..]),
            ("  fitting:", &ascii_lines, &[FITTING_WIDTH][..]),
        ] {
            let mut samples = Vec::new();
            let mut boundaries = 0;
            for round in 0..=ROUNDS {
                let start = Instant::now();
                boundaries = 0;
                for line in lines {
                    for &width in widths {
                        boundaries += wrapper
                            .wrap_line(
                                &[LineFragment::text(line)],
                                px(width),
                                IndentAdjustment::NoIndent,
                            )
                            .count();
                    }
                }
                if round > 0 {
                    samples.push(start.elapsed());
                }
            }
            println!("{name:<11} {}, {boundaries} boundaries", summary(samples));
        }
    });

    let window = cx
        .open_window(size(px(1000.), px(800.)), |_, cx| cx.new(|_| Empty))
        .unwrap();
    let mut shape_text = Vec::new();
    let mut wrapped_rows = 0;
    for round in 0..=ROUNDS {
        cx.update_window(window.into(), |_, window, cx| {
            let start = Instant::now();
            wrapped_rows = 0;
            for line in &lines[..SHAPED_LINES] {
                let runs = [TextRun {
                    len: line.len(),
                    font: text_font.clone(),
                    color: black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }];
                for width in WIDTHS {
                    let shaped = window
                        .text_system()
                        .shape_text(line.clone(), font_size, &runs, Some(px(width)), None)
                        .unwrap();
                    wrapped_rows += shaped
                        .iter()
                        .map(|line| line.wrap_boundaries().len() + 1)
                        .sum::<usize>();
                }
            }
            if round > 0 {
                shape_text.push(start.elapsed());
            }
            // Two frames, so the next round finds no wrapped line in the
            // frame caches and computes its boundaries again.
            window.draw(cx).clear(cx);
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }
    println!(
        "shape_text: {}, {SHAPED_LINES} lines, {wrapped_rows} rows",
        summary(shape_text)
    );
}
