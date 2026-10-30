> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# Sift

Sift is a lightweight, read-only code reader for macOS. Open a project, browse it,
search it and jump through it with full language-server navigation, without the
weight of a full IDE.

Sift is a fork of the [Zed](https://github.com/zed-industries/zed) editor, cut down
to the parts that help you read code.

## Based on Zed

Sift is based on Zed at commit
[`9e84b066b55bc3609a30dd965075c12255b38b45`](https://github.com/zed-industries/zed/commit/9e84b066b55bc3609a30dd965075c12255b38b45)
(2026-10-22, Zed version 1.27.0, the `nightly` tag at the time of forking).

All credit for the editor core, GPUI, language support and everything else Sift
still contains goes to [Zed Industries](https://zed.dev) and the Zed contributors.
Sift is not affiliated with or endorsed by Zed Industries.

## What changed from Zed

### Read-only by default

- Every file opens read-only, and the unlock toggle is removed.
- Sift's own configuration files stay editable: settings, keymap and themes in the
  config directory, and a project's `.zed` folder.

### Removed features

- AI: the agent panel, inline assistant, edit predictions, Copilot, language model
  providers, web search, agent skills and the agent sandbox.
- Collaboration: calls, voice and screen sharing (LiveKit/WebRTC), channels and the
  collab panel.
- Editing and running tools: the terminal, tasks, debugger, REPL and notebooks, Vim
  mode, snippets UI, journal and toolchain selector.
- Other panels and screens: the diagnostics panel, line-ending selector, onboarding,
  feedback, component preview, and developer tools such as the inspector, profiler
  and log views.
- Telemetry and auto-update are off by default.

The source of every removed feature was deleted from the repository. The Settings
window no longer shows settings for features that are gone.

### Navigation

- "Go to definition / implementation / references" with more than one target shows
  a popup list at the clicked symbol, like IntelliJ. You pick a target, then Sift
  jumps there.
- The list leaves out the location you started from. If only one target remains,
  Sift jumps straight to it.
- If "go to implementation" finds nothing, Sift falls back to the definition.
- Cmd-clicking a result opens it in the current pane instead of a split.

### Defaults and branding

- The JetBrains keymap is the default.
- The app is named Sift and has its own icon.
- Bundle identifiers use the `letus.cool` domain (`cool.letus.Sift`), and the URL
  scheme is `sift://`.
- The sidebar lists projects only. AI thread lists and thread search are removed.

## Building

Sift builds the same way as Zed. See
[Building Zed for macOS](./docs/src/development/macos.md) for the toolchain setup.

Run a development build:

```bash
cargo run
```

Build a release DMG at `target/aarch64-apple-darwin/release/Sift-aarch64.dmg`:

```bash
script/bundle-mac
```

Without Apple signing credentials, the DMG is signed only for the build machine and
is not notarized. Other Macs will ask for confirmation the first time Sift opens
(right-click the app, then choose Open).

## License

Sift is a derivative work of Zed and keeps Zed's licenses:

- GPL-3.0-or-later for the application ([LICENSE-GPL](./LICENSE-GPL)).
- Apache-2.0 for components marked as such, including GPUI
  ([LICENSE-APACHE](./LICENSE-APACHE)).

Each crate's `Cargo.toml` states its license. Third-party dependency licenses are
collected with [`cargo-about`](https://github.com/EmbarkStudios/cargo-about); see
`script/licenses/zed-licenses.toml`.
