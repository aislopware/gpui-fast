//! How much GPU time the window server spent compositing, from an Instruments trace
//! (macOS). Record the whole system while something runs, then read the trace:
//!
//! ```text
//! xcrun xctrace record --template 'Metal System Trace' --all-processes \
//!     --time-limit 10s --output /tmp/ws.trace
//! cargo run -p gpui_perf --example windowserver_gpu --release -- /tmp/ws.trace
//! ```
//!
//! It prints the time the window server's GPU work covered (overlapping intervals
//! counted once), per second of the trace and per refresh at `--refresh-hz` (120 by
//! default), and the same for every other process that used the GPU.

use std::collections::HashMap;

use smol::process::Command;

fn main() {
    let mut args = std::env::args().skip(1);
    let trace = args.next().expect("the path of a .trace recording");
    let mut refresh_hz = 120.;
    while let Some(arg) = args.next() {
        if arg == "--refresh-hz" {
            refresh_hz = args
                .next()
                .and_then(|hz| hz.parse().ok())
                .expect("a refresh rate in Hz");
        }
    }
    let export = |schema: &str| {
        let output = smol::block_on(
            Command::new("xcrun")
                .args(["xctrace", "export", "--input", &trace, "--xpath"])
                .arg(format!(
                    "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"{schema}\"]"
                ))
                .output(),
        )
        .expect("run xctrace export");
        assert!(output.status.success(), "xctrace export failed");
        String::from_utf8(output.stdout).expect("UTF-8 XML")
    };

    let intervals = rows(
        &export("metal-gpu-intervals"),
        &["start", "duration", "process"],
    );
    let mut by_process: HashMap<String, Vec<(u64, u64)>> = HashMap::new();
    let (mut first, mut last) = (u64::MAX, 0);
    for row in &intervals {
        let (Some(start), Some(duration)) =
            (row[0].1.parse::<u64>().ok(), row[1].1.parse::<u64>().ok())
        else {
            continue;
        };
        first = first.min(start);
        last = last.max(start + duration);
        let process = row[2].0.split(" (").next().unwrap_or_default().to_string();
        by_process
            .entry(process)
            .or_default()
            .push((start, start + duration));
    }
    let seconds = (last.saturating_sub(first)) as f64 / 1e9;
    let mut busy: Vec<(String, f64)> = by_process
        .into_iter()
        .map(|(process, intervals)| (process, union_ns(intervals) as f64 / 1e6))
        .collect();
    busy.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("trace {seconds:.2} s");
    for (process, ms) in busy {
        println!(
            "{process}: gpu_busy_ms_per_s={:.3} gpu_busy_ms_per_refresh={:.4}",
            ms / seconds,
            ms / seconds / refresh_hz
        );
    }
}

/// The time the intervals cover, counting overlaps once.
fn union_ns(mut intervals: Vec<(u64, u64)>) -> u64 {
    intervals.sort_unstable();
    let mut total = 0;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in intervals {
        match &mut current {
            Some((_, current_end)) if start <= *current_end => {
                *current_end = (*current_end).max(end)
            }
            _ => {
                if let Some((current_start, current_end)) = current {
                    total += current_end - current_start;
                }
                current = Some((start, end));
            }
        }
    }
    total + current.map_or(0, |(start, end)| end - start)
}

/// The named columns of every row of an exported table, as `(fmt, text)` pairs,
/// following the export's `ref` attributes back to the element they repeat.
fn rows(xml: &str, columns: &[&str]) -> Vec<Vec<(String, String)>> {
    let document = roxmltree::Document::parse(xml).expect("xctrace's XML");
    let schema = document
        .descendants()
        .find(|node| node.has_tag_name("schema"))
        .expect("a table schema");
    let mnemonics: Vec<String> = schema
        .children()
        .filter(|node| node.has_tag_name("col"))
        .map(|col| {
            col.children()
                .find(|node| node.has_tag_name("mnemonic"))
                .and_then(|node| node.text())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let wanted: Vec<usize> = columns
        .iter()
        .map(|column| {
            mnemonics
                .iter()
                .position(|mnemonic| mnemonic == column)
                .unwrap_or_else(|| panic!("no column {column}"))
        })
        .collect();
    let mut by_id: HashMap<&str, (String, String)> = HashMap::new();
    let mut rows = Vec::new();
    for row in document
        .descendants()
        .filter(|node| node.has_tag_name("row"))
    {
        let mut values = Vec::new();
        for cell in row.children().filter(|node| node.is_element()) {
            // Every element with an id may be referred to later, nested ones included.
            for node in cell.descendants().filter(|node| node.is_element()) {
                if let Some(id) = node.attribute("id") {
                    by_id.insert(
                        id,
                        (
                            node.attribute("fmt").unwrap_or_default().to_string(),
                            node.text().unwrap_or_default().to_string(),
                        ),
                    );
                }
            }
            let value = match cell.attribute("ref") {
                Some(reference) => by_id.get(reference).cloned().unwrap_or_default(),
                None => (
                    cell.attribute("fmt").unwrap_or_default().to_string(),
                    cell.text().unwrap_or_default().to_string(),
                ),
            };
            values.push(value);
        }
        rows.push(
            wanted
                .iter()
                .map(|&index| values.get(index).cloned().unwrap_or_default())
                .collect(),
        );
    }
    rows
}
