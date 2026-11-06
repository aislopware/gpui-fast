# GPUI Fast

> [!NOTE]
> **aislopware/gpui-fast** is the GPUI that [onicode](https://github.com/aislopware/onicode) renders with:
> [longbridge/gpui-fast](https://github.com/longbridge/gpui-fast) plus zed imports ahead of longbridge, native
> composition for embedded WebKit, presentation reports and pacing, scroll layers, edge fades, backdrop blur, and
> many retained-mode and macOS fixes. Onicode pins a rev on `main`. Upstream changes are read one by one and taken
> by intent, never merged blindly; [FORK.md](FORK.md) records every sync, what the fork carries and why.

A performance-focused fork of [Zed's GPUI](https://gpui.rs) with incremental rendering and native window composition. [v0.1.0](https://github.com/longbridge/gpui-fast/releases/tag/v0.1.0) is available on crates.io. The project remains experimental.

- **Retained Mode** reuses unchanged views, layout nodes and text measurements.
- **Scroll layers** reuse cached GPU tiles and list rows while scrolling, on macOS, Linux and Windows.
- **Window composition** embeds native content while keeping GPUI menus, popovers and dialogs above it.

## Retained Mode

GPUI Fast tracks rendering dependencies and rebuilds only what changed. Recorded Linux release-build CPU time per frame:

| Workload | Baseline | GPUI Fast |
| --- | --- | --- |
| Unchanged window | 3.31 ms | 0.19 ms (−94%) |
| Sidebar scrolling | 5.95 ms | 0.79 ms (−87%) |
| Streaming quotes | 5.06 ms | 1.35 ms (−73%) |

The unchanged-window baseline disables retention in the same build; the other rows compare against upstream `gpui-pre` 0.3.7. Results depend on the workload. See [Retained Mode](docs/retained-mode.md) and [Scroll layers](docs/scroll-layers.md) for measurements and implementation details.

## Window composition

Opt-in composition supports macOS, Windows, Linux Wayland and X11, based on [zed#62379](https://github.com/zed-industries/zed/pull/62379).

```sh
cargo run -p gpui_perf --example native_webview
```

The example embeds WKWebView on macOS and WebView2 on Windows. Linux demonstrates an independent GPU surface rather than a WebView.

## Using it

Alias the published crates to GPUI's library names:

```toml
[dependencies]
gpui = { package = "gpui-fast", version = "0.1.0" }
gpui_platform = { package = "gpui-fast-platform", version = "0.1.0", features = ["wayland", "x11"] }
```

The platform features above select Linux backends; omit them on macOS and Windows. GPUI Kit applications should follow the [compatibility setup](compat/README.md) to use the same GPUI core.

Changes to render state outside tracked entities, globals and list/scroll state need `cx.notify()`. See [Retained Mode](docs/retained-mode.md#what-an-application-needs-to-know) for details.

## Following upstream

v0.1.0 includes Zed's GPUI through [`a1b71072e5`](https://github.com/zed-industries/zed/commit/a1b71072e5b43faef437b471e988fbb5f972c99c). [UPSTREAM](UPSTREAM) records the revision for the current checkout.

GPUI Fast keeps its implementation in `fast/` modules with small hooks into upstream code, making future syncs easier. See [Architecture](docs/architecture.md) and [Contributing](CONTRIBUTING.md) for design, building and testing.

## License

Apache-2.0 — copyright Zed Industries, Inc. This is a modified fork; see [LICENSE-APACHE](LICENSE-APACHE).
