<<<<<<< HEAD
# OpenFlow
# VIBE-CODED
Push-to-talk dictation for macOS. Hold a hotkey, speak, release — your words are transcribed in the cloud, cleaned up by an LLM, and pasted at your cursor. Built with Tauri 2, React and TypeScript.
=======
# OpenFlow
# VIBE-CODED

Cross-platform voice dictation, inspired by [Wispr Flow](https://wisprflow.ai). Hold a hotkey, speak naturally, and polished text lands at your cursor in any app — with filler words removed, punctuation fixed, and self-corrections resolved.

Built with **Tauri 2** (Rust core + React/TypeScript UI), targeting **macOS and Windows**.
>>>>>>> 1b4e83e (Add voice commands, error auto-dismiss, and README (Milestone 6))

## Features

- **Push-to-talk dictation** — hold the hotkey (default `F5`), speak, release. Text is transcribed, cleaned up by an LLM, and pasted at your cursor.
- **Hands-free mode** — double-tap the hotkey to keep recording without holding; tap again or press Esc to finish.
- **AI cleanup** — GPT-4o-mini (OpenAI) or Claude (Anthropic) rewrites raw speech into clear prose: filler removal, punctuation, spoken formatting ("new paragraph", "numbered list"), backtrack handling.
- **Pluggable LLM providers** — bring an OpenAI key, a Claude key, or both; auto-detect or force a provider per your preference.
- **Voice commands** — say "open youtube", "search rust async", or "copy …" to act instead of typing (toggleable).
- **Personal dictionary** — teach it names and jargon; starred terms get priority; misspelling rules auto-correct.
- **Snippets** — voice shortcuts that expand to full text locally, with zero API latency on exact matches.
- **Per-app styles** — tone instructions matched against the frontmost app's bundle identifier (e.g., formal in Mail, casual in Slack).
- **Flow Bar** — floating, focus-safe pill with live waveform, drag-to-position, click-to-dictate.
- **History & stats** — searchable transcript history grouped by day, word counts, streaks.
- **Multi-language** — 19 languages plus auto-detect for transcription.
- **Customizable hotkey** — any of F1–F12, CapsLock, or right-side modifiers; applies live.
- **Guided onboarding** — first-launch wizard walks through Accessibility + microphone permissions with live checks, then a real dictation test unlocks the app.
- **Privacy-first storage** — everything local in SQLite; audio is transient; keys stay on-device.

## Architecture

```
┌─────────────────── React UI (webview windows) ───────────────────┐
│  Hub: Home/History · Dictionary · Snippets · Style · Settings    │
│  Flow Bar: transparent always-on-top dictation pill              │
└────────────────────────────┬─────────────────────────────────────┘
                             │ Tauri IPC commands + events
┌────────────────────────────▼───────────── Rust Core ─────────────┐
│ hotkey.rs    global push-to-talk watcher (tap vs hold detection) │
│ audio.rs     cpal mic capture → 16-bit WAV + RMS level events    │
│ pipeline.rs  FSM: idle → recording → transcribing → injecting    │
│ cloud/stt.rs OpenAI transcription (gpt-4o-transcribe)            │
│ cloud/llm.rs Cleanup pass — OpenAI Chat / Anthropic Messages     │
│ commands.rs  Voice command parser + executor                     │
│ inject/      Clipboard-paste: System Events (macOS), SendInput    │
│              Ctrl+V (Windows)                                    │
│ store.rs     SQLite: transcripts, dictionary, snippets, styles   │
└───────────────────────────────────────────────────────────────────┘
```

### Dictation flow

1. Hotkey down → frontmost app captured for style matching, mic stream opens.
2. Hotkey up → WAV encoded to a temp file.
3. OpenAI STT transcribes (dictionary terms injected as a bias prompt).
4. Snippet fast-path: exact trigger match expands locally, no LLM call.
5. LLM cleanup polishes the text (raw text is the fallback if this fails — you never lose a dictation).
6. Command mode check: recognized commands execute instead of pasting.
7. Text is staged on the clipboard, Cmd+V synthesized into the target app, clipboard restored.

## Getting started

Prerequisites: Node 20+, Rust stable, Xcode CLT (macOS).

```bash
npm install
npm run tauri dev
```

First run:

1. Grant **Accessibility** permission when prompted (Settings → Privacy & Security → Accessibility) — required for global hotkeys, paste injection, and app detection.
2. Grant **Microphone** access on first dictation.
3. Add an API key in **Settings → API keys** (OpenAI for transcription; OpenAI or Claude for cleanup). Env vars `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` also work.

Then hold `F5` anywhere and talk.

### Platform notes

- **macOS** — requires Accessibility permission (global hotkey, paste injection, frontmost-app detection) and Microphone permission. Paste is performed by staging the clipboard and synthesizing Cmd+V via System Events; the clipboard is restored ~800 ms later.
- **Windows** — no permission prompts needed. Paste uses SendInput to synthesize Ctrl+V with the same clipboard save/restore dance. Known limitation: injection cannot reach apps running elevated (as administrator) unless FlowClone is elevated too. Frontmost-app detection returns the process name (e.g. `chrome`), which per-app styles match against.

### Production build & releases

```bash
npm run tauri build          # local bundle (NSIS/MSI on Windows, .app on macOS)
```

Releases are automated: push a tag (`git tag v0.1.0 && git push origin v0.1.0`) and CI builds signed bundles for macOS (Apple Silicon + Intel) and Windows via `tauri-action`, publishing them to a draft GitHub Release along with `latest.json` — the manifest the in-app updater polls.

One-time setup for that pipeline:

1. **Updater keypair**: `npm run tauri signer generate -w ~/.tauri/openflow.key`. Put the **public** key into `src-tauri/tauri.conf.json → plugins.updater.pubkey`, and the private key + password into repo secrets `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
2. **macOS signing/notarization** (optional for personal use): set `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` repo secrets. Unsigned mac builds still run locally; notarized ones install cleanly on other machines.
3. In-app updates land in **Settings → Check for updates**.

## Testing

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # unit tests (store, audio, hotkey, llm, commands)
cargo clippy --manifest-path src-tauri/Cargo.toml # lint gate (-D warnings)
npm run build                                     # TS typecheck + vite build
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy, tests, and frontend builds on both macOS and Windows runners; tags (`v*`) cut signed release bundles via tauri-action.

## Milestones

| # | Scope | Status |
|---|-------|--------|
| 1 | Tauri + React scaffold, SQLite schema, Hub shell | ✅ |
| 2 | Core loop: hotkey → capture → STT → paste | ✅ |
| 3 | Flow Bar window, live waveform, toggle/cancel | ✅ |
| 4 | LLM cleanup (OpenAI + Claude), snippets, styles | ✅ |
| 5 | Hands-free, hotkey customization, languages, autostart | ✅ |
| 6 | Voice commands, error auto-dismiss polish | ✅ |
| 7 | Windows port (SendInput injection, frontmost app) | ✅ |
| 8 | Onboarding wizard + permission gates | ✅ |
| 9 | Packaging, updater, signing pipeline | ✅ |

## License

MIT
