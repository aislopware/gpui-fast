//! What system glass under a sidebar costs: a 1280×800 window with a 260 pt sidebar and an opaque
//! content area that animates every refresh, run for a fixed time with the window opaque or
//! blurred (`glass_cost opaque|blurred [animate|idle] [seconds]`). Prints the frames drawn, the
//! intervals between them and the process's and WindowServer's CPU time are read around the
//! run from outside.

use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;

struct Glass {
    blurred: bool,
    animate: bool,
    started: Instant,
    last: Option<Instant>,
    intervals: Vec<Duration>,
    run: Duration,
}

impl Render for Glass {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        if let Some(last) = self.last {
            self.intervals.push(now - last);
        }
        self.last = Some(now);
        if now - self.started >= self.run {
            report(&mut self.intervals, self.run);
            cx.quit();
        } else if self.animate {
            window.request_animation_frame();
        } else {
            let entity = cx.entity();
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let _ = entity.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        let t = (now - self.started).as_secs_f32();
        let x = (t * 2.0).sin().mul_add(300.0, 360.0);
        let sidebar = div()
            .w(px(260.))
            .h_full()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(12.))
            .when(!self.blurred, |el| el.bg(rgb(0xececec)))
            .when(self.blurred, |el| el.bg(rgba(0xececec18)))
            .children((0..28).map(|i| div().h(px(20.)).text_sm().child(format!("Row {i}"))));
        let content = div()
            .flex_1()
            .h_full()
            .bg(rgb(0xfaf9f7))
            .relative()
            .children((0..40).map(|i| {
                div()
                    .text_xs()
                    .child(format!("{i:03} the quick brown fox jumps over the lazy dog 0123456789"))
            }))
            .child(
                div()
                    .absolute()
                    .top(px(300.))
                    .left(px(x))
                    .size(px(80.))
                    .rounded(px(12.))
                    .bg(rgb(0x3d8b5a)),
            );
        div().size_full().flex().flex_row().child(sidebar).child(content)
    }
}

fn report(intervals: &mut Vec<Duration>, run: Duration) {
    intervals.sort_unstable();
    let pick = |q: f64| {
        let ix = ((intervals.len() as f64 - 1.0) * q).round() as usize;
        intervals.get(ix).map_or(0.0, |d| d.as_secs_f64() * 1e3)
    };
    println!(
        "GLASS frames {} in {:.0} s, interval p50 {:.2} p95 {:.2} p99 {:.2} ms",
        intervals.len() + 1,
        run.as_secs_f64(),
        pick(0.5),
        pick(0.95),
        pick(0.99)
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let blurred = args.get(1).is_some_and(|a| a == "blurred");
    let animate = args.get(2).is_none_or(|a| a == "animate");
    let run = Duration::from_secs(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20));
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                focus: false,
                // Unfocused, as a measurement must leave the person's focus alone, but drawn at
                // the display's rate as the focused window would be.
                inactive_frame_interval: None,
                window_background: if blurred {
                    WindowBackgroundAppearance::Blurred
                } else {
                    WindowBackgroundAppearance::Opaque
                },
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|_| Glass {
                    blurred,
                    animate,
                    started: Instant::now(),
                    last: None,
                    intervals: Vec::new(),
                    run,
                })
            },
        )
        .expect("a window");
    });
}
