# OpenFlow
# VIBE-CODED
Push-to-talk dictation for macOS. Hold a hotkey, speak, release — your words are transcribed in the cloud, cleaned up by an LLM, and pasted at your cursor. Built with Tauri 2, React and TypeScript.

## Features

- **Push-to-talk** — hold the hotkey (`F5` by default) to record, release to transcribe and paste.
- **Hands-free mode** — quick double-tap the hotkey to keep recording without holding it; tap again to finish. Sessions are capped at 6 minutes.
- **FlowBar** — a floating, always-on-top pill that shows live audio levels while recording, progress while transcribing/injecting, and can start/stop dictation with a click. Drag it anywhere; its position is remembered.
- **Transcription** — OpenAI speech-to-text (`gpt-4o-transcribe`) with dictionary terms used as vocabulary biasing and auto-detect or manual language selection.
- **LLM cleanup** — fillers, false starts and punctuation are fixed before pasting via OpenAI (`gpt-4o-mini`) or Anthropic Claude (`claude-3-5-haiku-latest`). If cleanup is unavailable, the raw transcript is pasted anyway so nothing is lost.
- **Snippets** — say a trigger phrase (e.g. "my email") and its full text is expanded instantly, skipping transcription cleanup entirely.
- **Per-app styles** — pattern-match the frontmost app's bundle identifier to apply different tone/formatting instructions per app.
- **Configurable hotkey & launch at login** — pick any function key, Caps Lock, or a right-side modifier in Settings; optionally start OpenFlow when you sign in.
- **History** — every session is stored locally in SQLite with raw + polished text, duration, and target app.

## Requirements

- macOS 12+
- **Microphone** permission (prompted on first recording)
- **Accessibility** permission — required for global hotkey capture, pasting at the cursor, and frontmost-app detection. Grant it in System Settings → Privacy & Security → Accessibility (the Hub links directly there).
- An **OpenAI API key** (`sk-…`) — used for transcription and cleanup
- Optional: an **Anthropic API key** to use Claude for the cleanup step instead

Keys can be entered in Hub → Settings or provided via the `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` environment variables.

## Getting started

```sh
npm install
npm run tauri dev     # run the app in development
```

Production build:

```sh
npm run tauri build
```

Run the Rust test suite:

```sh
cargo test            # from src-tauri/
```

## Architecture

| Piece | Where | What it does |
| --- | --- | --- |
| Hub | `src/hub/` | Main window: Home, Dictionary, Snippets, Style, Settings |
| FlowBar | `src/flowbar/` | Floating status bar window |
| Pipeline | `src-tauri/src/pipeline.rs` | State machine: idle → recording → transcribing → injecting |
| Audio | `src-tauri/src/audio.rs` | Microphone capture (cpal) + WAV encoding, RMS level events |
| Hotkeys | `src-tauri/src/hotkey.rs` | Global push-to-talk polling (`device_query`) |
| STT / LLM | `src-tauri/src/cloud/` | OpenAI transcription, LLM cleanup, snippet fast path |
| Injection | `src-tauri/src/inject/` | Clipboard staging + Cmd+V synthesis, clipboard restored after paste |

The pipeline degrades gracefully: a failed LLM call falls back to the raw transcript, and a failed paste still saves the text to history.
