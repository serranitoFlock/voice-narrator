# VoiceToggle (cross-platform)

Tray app that controls the `~/voice-pipeline` text-to-speech pipeline. It is the
Tauri v2 port of the macOS-only Swift app in `../VoiceToggleApp`, targeting
macOS, Windows, and Linux from a single codebase.

## Contract

The app holds no state of its own. Every setting is a dotfile in
`~/voice-pipeline`, shared with the shell scripts and the Swift app:

| File                  | Meaning                                    | Default    |
| --------------------- | ------------------------------------------ | ---------- |
| `.disabled`           | Present = voice muted                      | absent     |
| `.engine`             | `kokoro` or `piper`                        | `kokoro`   |
| `.kokoro-voice`       | Kokoro voice code                          | `em_santa` |
| `.piper-voice`        | `.onnx` file name in `models/piper/`       | only model |
| `.retention-days`     | Marker history retention (days)            | `7`        |
| `.log-retention-days` | `hook.log` retention (days)                | `14`       |

The tray polls these files every 2 seconds, so changes made by scripts show up
in the menu.

## Features

- Toggle voice from the menu or with the global shortcut: `⌃⌥⌘V` on macOS,
  `Ctrl+Alt+Win+V` on Windows, `Ctrl+Alt+Super+V` on Linux.
- Pick engine, Kokoro voice, Piper voice, and cleanup retention.
- "Editar diccionario..." opens `scripts/en-respellings.sed` in a text editor
  (a native editor window is pending).

## Development

Requires Rust (stable) and Node.js.

```sh
npm install          # Tauri CLI, only needed for bundling
npm run dev          # run the tray app
npm test             # config contract tests
npm run build        # bundle (.app/.dmg, .msi/.exe, .deb/.AppImage)
```

Bundles must be built on each target OS (or in CI).

## Platform notes

- Linux: the tray uses AppIndicator/StatusNotifier. GNOME needs the
  "AppIndicator and KStatusNotifierItem Support" extension.
- Windows/Linux: the TTS pipeline itself (`scripts/*.sh`, `afplay`) is still
  macOS-only; porting it is a separate task.
- macOS: while the Swift app is running, its event tap consumes `⌃⌥⌘V` first.
