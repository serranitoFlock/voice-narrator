# voice-pipeline (cross-platform)

Cross-platform port of the `~/voice-pipeline` text-to-speech pipeline that
narrates Claude Code responses, targeting macOS, Windows, and Linux from a
single Rust codebase:

- `core/` (`voice-core`): config, dictionary, text cleanup, dispatch queue,
  dedup, TTS engines and audio. Builds the `voice-speak` binary that replaces
  `scripts/speak.sh`, `say.sh`, `voice.sh` and `engine.sh`.
- `src-tauri/` (`voice-toggle`): the tray app, a Tauri v2 port of the
  macOS-only Swift app in `../VoiceToggleApp`.

## voice-speak

```sh
voice-speak               # Claude Code hook: reads the payload on stdin
voice-speak say "Hola"    # speak now
voice-speak clean "text"  # show what the engine would receive
voice-speak on|off|status
voice-speak engine kokoro|piper|status
```

Hook configuration (`~/.claude/settings.json`), same events as `speak.sh`:
`Stop`, `PostToolUse` (no matcher) and `PreToolUse` with matcher
`AskUserQuestion`, each running the `voice-speak` binary with `async: true`.

External requirements: Kokoro (`kokoro-venv` + `scripts/kokoro-say.py`) and/or
Piper (`~/.local/bin/piper` or on `PATH`). `sox`, `afplay`, `lockf`, `perl` and
BSD `sed` are no longer needed. Set `VOICE_PIPELINE_DIR` to use a pipeline
directory other than `~/voice-pipeline`.

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
- "Editar diccionario..." opens an editor window for
  `scripts/en-respellings.sed`: click a row to edit it, add new words, delete
  with a two-step confirmation. Words and respellings cannot contain `/` or
  line breaks, since they would produce an invalid sed rule. Saving rewrites
  the file with one block per word, sorted alphabetically; the header and
  every rule's comment history are preserved.

## Install (macOS)

```sh
npx tauri build --bundles app
cp -R target/release/bundle/macos/VoiceToggle.app ~/Applications/
cp target/release/voice-speak ~/voice-pipeline/bin/.voice-speak.new \
  && mv -f ~/voice-pipeline/bin/.voice-speak.new ~/voice-pipeline/bin/voice-speak
```

Replace `voice-speak` with `mv`, never by copying over it: a dispatcher may be
running from the old file. The tray app turns "Abrir al iniciar sesión" on the
first time it runs (a LaunchAgent on macOS, the registry on Windows, an XDG
autostart entry on Linux); the menu item turns it off again.

## CI

`.github/workflows/build.yml` tests and builds on macOS, Windows and Linux,
and uploads `voice-speak` plus the tray bundles (`.dmg`, `.msi`/setup `.exe`,
`.deb`/`.AppImage`) as artifacts.

## Development

Requires Rust (stable) and Node.js.

```sh
npm install          # Tauri CLI, only needed for bundling
npm run dev          # run the tray app
npm test             # config and dictionary tests
cargo test --manifest-path src-tauri/Cargo.toml -- --ignored  # round-trip of the real dictionary
npm run build        # bundle (.app/.dmg, .msi/.exe, .deb/.AppImage)
```

Bundles must be built on each target OS (or in CI).

## Platform notes

- Linux: the tray uses AppIndicator/StatusNotifier. GNOME needs the
  "AppIndicator and KStatusNotifierItem Support" extension.
- Windows: the Kokoro venv is expected at `kokoro-venv/Scripts/python.exe`.
- Linux: building needs the ALSA development package (`libasound2-dev`).
- macOS: while the Swift app is running, its event tap consumes `⌃⌥⌘V` first.
