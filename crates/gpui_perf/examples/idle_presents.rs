//! An idle window notified by a timer that changes nothing it shows, as a
//! clock ticking every second shows the same minute: how many frames it
//! presents and what CPU it takes while idle (`fast::same_frame`).
//!
//! The window shows a few labels and a clock. A timer notifies the view
//! `--hz` times a second; every `--change-every`th tick the clock's text
//! changes, and the ticks between draw exactly the frame before. It opens a
//! floating window, visible over the others, without taking focus, runs for `--secs` seconds after a second's
//! warm-up, prints one line and quits.
//!
//! ```text
//! cargo run -p gpui_perf --example idle_presents --release -- 20 10 10
//! GPUI_SAME_FRAME=0 cargo run -p gpui_perf --example idle_presents --release -- 20 10 10
//! ```
//!
//! The arguments are seconds measured, ticks a second and the ticks between
//! two changes (0: never change). `GPUI_SAME_FRAME=0` presents every frame
//! drawn, as before the skip, for the same binary to compare.

extern crate gpui_fast as gpui;
extern crate gpui_platform_fast as gpui_platform;

use gpui::{
    Bounds, Context, PresentCounts, Render, SharedString, Window, WindowBounds, WindowKind,
    WindowOptions, div, hsla, prelude::*, px, size,
};
use gpui_platform::application;
use std::time::{Duration, Instant};

/// Time drawn before the measurement starts.
const WARMUP: Duration = Duration::from_secs(1);

struct Clock {
    tick: u64,
    change_every: u64,
    draws: u64,
}

impl Render for Clock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.draws += 1;
        let shown = self.tick.checked_div(self.change_every).unwrap_or(0);
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(hsla(0., 0., 0.12, 1.))
            .text_color(hsla(0., 0., 0.85, 1.))
            .children((0..12).map(|row| {
                div()
                    .h(px(20.))
                    .bg(hsla(0.6, 0.2, 0.2 + row as f32 * 0.02, 1.))
                    .child(SharedString::from(format!("row {row}")))
            }))
            .child(SharedString::from(format!("12:{:02}", shown % 60)))
    }
}

/// CPU time the whole process has used so far, user and system.
fn process_cpu_time() -> Duration {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` fills in the whole struct when it returns 0.
    let usage = unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) != 0 {
            return Duration::ZERO;
        }
        usage.assume_init()
    };
    let time = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    time(usage.ru_utime) + time(usage.ru_stime)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let secs: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(20);
    let hz: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);
    let change_every: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(10);

    application().run(move |cx| {
        cx.open_window(
            WindowOptions {
                focus: false,
                // Above other windows, so that it is visible and draws, without taking focus.
                kind: WindowKind::PopUp,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(480.), px(420.)),
                    cx,
                ))),
                inactive_frame_interval: None,
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    cx.spawn_in(window, async move |this, cx| {
                        let executor = cx.background_executor().clone();
                        let period = Duration::from_secs(1) / hz.max(1) as u32;
                        let mut start: Option<(Instant, Duration, u64, PresentCounts)> = None;
                        loop {
                            executor.timer(period).await;
                            let Ok(done) = this.update_in(cx, |clock: &mut Clock, window, cx| {
                                clock.tick += 1;
                                cx.notify();
                                let counts = window.present_counts();
                                match start {
                                    None if clock.tick * period.as_millis() as u64
                                        >= WARMUP.as_millis() as u64 =>
                                    {
                                        start = Some((
                                            Instant::now(),
                                            process_cpu_time(),
                                            clock.draws,
                                            counts,
                                        ));
                                        false
                                    }
                                    Some((at, cpu, draws, since))
                                        if at.elapsed() >= Duration::from_secs(secs) =>
                                    {
                                        let elapsed = at.elapsed().as_secs_f64();
                                        let cpu = (process_cpu_time() - cpu).as_secs_f64();
                                        let presented = counts.presented - since.presented;
                                        let skipped = counts.skipped - since.skipped;
                                        println!(
                                            "idle_presents: {secs} s at {hz} ticks/s, change every \
                                             {change_every}: draws {}, presented {presented} \
                                             ({:.2}/s), skipped {skipped}, cpu {:.2}% \
                                             ({:.1} ms/s), same_frame {}",
                                            clock.draws - draws,
                                            presented as f64 / elapsed,
                                            cpu / elapsed * 100.,
                                            cpu / elapsed * 1e3,
                                            std::env::var("GPUI_SAME_FRAME").unwrap_or("on".into()),
                                        );
                                        cx.quit();
                                        true
                                    }
                                    _ => false,
                                }
                            }) else {
                                break;
                            };
                            if done {
                                break;
                            }
                        }
                    })
                    .detach();
                    Clock {
                        tick: 0,
                        change_every,
                        draws: 0,
                    }
                })
            },
        )
        .unwrap();
    });
}
