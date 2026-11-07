# gpui-pre compatibility crates

[GPUI Kit](https://github.com/longbridge/gpui-kit) depends on GPUI through the
`gpui-pre-*` snapshots on crates.io, and Cargo's `[patch]` only replaces a
package with one of the same name and a version that meets the requirement.
The crates here carry exactly those names and the version GPUI Kit pins, and
each re-exports this repository's crate of the same library name
(`pub use gpui_fast::*`), so an application on GPUI Kit, from crates.io or git
alike, runs on gpui-fast with:

```toml
[patch.crates-io]
gpui-pre = { git = "https://github.com/longbridge/gpui-fast" }
gpui-pre-platform = { git = "https://github.com/longbridge/gpui-fast" }
gpui-pre-macros = { git = "https://github.com/longbridge/gpui-fast" }
gpui-pre-sum-tree = { git = "https://github.com/longbridge/gpui-fast" }
# and gpui-pre-reqwest-client when the graph has it
```

GPUI Kit's `gpui-fast` feature names GPUI Fast's crates.io release instead
(`gpui-fast`, `gpui-fast-platform`, `0.1.13` today), and `gpui-fast` and
`gpui-fast-platform` here stand in for those the same way, so that feature
builds on this repository too:

```toml
[patch.crates-io]
gpui-fast = { git = "https://github.com/longbridge/gpui-fast" }
gpui-fast-platform = { git = "https://github.com/longbridge/gpui-fast" }
```

Each crate exposes the same features as the crate it stands in for,
forwarded to ours, except the Linux, Windows and web ones (`wayland`, `x11`,
`windows-manifest*`, `multithreaded`) and the web crates: this fork builds for
macOS and iOS only (FORK.md, "Apple only"). Nothing else lives here:
gpui-fast's own crates keep upstream's names and versions.

Two things to keep in mind:

- The version is the snapshot GPUI Kit pins, `=0.3.11` today, and the GPUI
  Fast release it names, `0.1.13`. When GPUI Kit moves to newer ones, move the
  crates here with them; until then Cargo warns that the patch was not used
  and keeps the crates.io crate.
- gpui-pre rewrites the `gpui::` paths its macros emit to `::gpui_kit::`, and
  gpui-fast's macros do not. A crate that depends on GPUI Kit alone aliases it
  at its root: `extern crate gpui_kit as gpui;`.
