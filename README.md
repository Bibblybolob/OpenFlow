# OpenFlow

Push-to-talk voice dictation for macOS and Windows. Hold a hotkey, speak naturally, release — your words are transcribed in the cloud, cleaned up by an LLM (filler words removed, punctuation fixed, self-corrections resolved), and pasted at your cursor in any app.

Built with **Tauri 2** (Rust core + React/TypeScript UI).

## Features

- **Push-to-talk dictation** — hold the hotkey (default `Right Shift`), speak, release. Text is transcribed, cleaned up by an LLM, and pasted at your cursor.
- **Hands-free mode** — double-tap the hotkey to keep recording without holding; tap again or press Esc to finish.
- **AI cleanup** — GPT-4o-mini (OpenAI), Claude (Anthropic), or any model via OpenRouter rewrites raw speech into clear prose: filler removal, punctuation, spoken formatting ("new paragraph", "numbered list"), backtrack handling.
- **Pluggable LLM providers** — bring an OpenAI, Claude, OpenRouter, or mix of keys; auto-detect or force a provider per your preference.
- **Voice commands** — say "open youtube", "search rust async", or "copy …" to act instead of typing (toggleable).
- **Personal dictionary** — teach it names and jargon; starred terms get priority; misspelling rules auto-correct.
- **Snippets** — voice shortcuts that expand to full text locally, with zero API latency on exact matches.
- **Per-app styles** — tone instructions matched against the frontmost app's bundle identifier (e.g., formal in Mail, casual in Slack).
- **Flow Bar** — floating, focus-safe pill with an always-on live waveform (real mic levels, independent of the animations setting) plus a "mic silent?" alert when the capture stream hears nothing for 2s — so you always know whether the pill is actually listening; click-to-dictate; drag it anywhere or snap it to screen-edge presets (remembered across restarts). Hide it when idle (it pops in only while dictating), and customize shape, accent color, opacity, and animations in Settings.
- **History & stats** — searchable transcript history grouped by day, word counts, streaks.
- **Multi-language** — 19 languages plus auto-detect for transcription.
- **Customizable hotkey** — any of F1–F12, CapsLock, or right-side modifiers (default **Right Shift**: under both palms, never types a character, and never intercepted by macOS features the way F5 is); applies live and migrates stale key names on upgrade.
- **Self-healing hotkey watcher** — if Input Monitoring is revoked (e.g. after replacing the app bundle), the watcher reports its state to the Hub ("waiting for permission / active / unavailable") and recovers automatically once the permission returns; microphone failures surface on the pill instead of silently doing nothing.
- **Reliable Flow Bar visibility** — the pill is shown natively by the Rust core on every state change (not just via webview events), its position is clamped to the visible monitor, and the webview reconciles against the pipeline state as a fallback — so dictation always has a visible indicator.
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
│ cloud/stt.rs OpenAI transcription (gpt-4o-transcribe) or          │
│              OpenRouter audio-input chat models                   │
│ cloud/llm.rs Cleanup pass — OpenAI Chat / Anthropic Messages     │
│ commands.rs  Voice command parser + executor                     │
│ inject/      Clipboard-paste: System Events (macOS), SendInput    │
│              Ctrl+V (Windows)                                    │
│ store.rs     SQLite: transcripts, dictionary, snippets, styles   │
└───────────────────────────────────────────────────────────────────┘
```

### Dictation flow

1. Hotkey down → frontmost app captured for style matching, mic stream opens.
2. Hotkey up → audio is resampled to 16 kHz mono and encoded as WAV in memory.
3. Transcription: OpenAI STT (gpt-4o-mini-transcribe by default — ~2x faster
   than the full model at near-identical quality), OpenRouter audio model, or
   on-device whisper.cpp (dictionary terms injected as a bias prompt).
   Connections are pre-warmed at launch and stay warm between sessions, so no
   TLS handshake is paid per dictation.
4. Snippet fast-path: exact trigger match expands locally, no LLM call.
5. LLM cleanup polishes the text (skippable via Settings → Cleanup; short
   utterances under ~120 chars skip it automatically via "Fast path", and raw
   text is always the fallback if cleanup fails — you never lose a dictation).
6. Command mode check: recognized commands execute instead of pasting.
7. Text is staged on the clipboard, Cmd+V synthesized natively via CGEvent
   (macOS) or SendInput Ctrl+V (Windows) into the target app, clipboard
   restored.

Processing runs on a worker thread, so the hotkey stays responsive while a
dictation is transcribing — a press during that window queues the next
session automatically. Per-stage timings are logged to stderr and emitted as
`pipeline-timing` events for latency profiling.

## Getting started

Prerequisites: Node 20+, Rust stable, Xcode CLT (macOS).

```bash
npm install
npm run tauri dev
```

First run:

1. Grant **Accessibility** permission when prompted (Settings → Privacy & Security → Accessibility) — required for paste injection and app detection.
2. Grant **Input Monitoring** permission (Settings → Privacy & Security → Input Monitoring) — required for the global hotkey watcher. The Hub shows live status for both and can open the right pane.
3. Grant **Microphone** access on first dictation.
4. Add an API key in **Settings → API keys** (OpenAI for transcription; OpenAI, Claude, or OpenRouter for cleanup). Env vars `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `OPENROUTER_API_KEY` also work.

Then hold `Right Shift` anywhere and talk.

> **Note:** replacing the app bundle (e.g., rebuilding or updating an ad-hoc-signed build) silently revokes Accessibility + Input Monitoring on macOS. If the hotkey stops working after an update, re-toggle both permissions.

### Platform notes

- **macOS** — requires Accessibility permission (paste injection, frontmost-app detection), Input Monitoring permission (global hotkey), and Microphone permission. Paste is performed by staging the clipboard and synthesizing Cmd+V via System Events; the clipboard is restored ~800 ms later. Hotkey detection uses a polling watcher gated on both permissions; it starts automatically once they are granted.
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

## Troubleshooting

### macOS: "FlowClone is damaged and can't be opened" / no "Open Anyway"

Release DMGs are currently **ad-hoc signed** (no Apple Developer certificate), so Gatekeeper's download-provenance check fails and macOS may refuse to open the app *without* offering "Open Anyway". Remove the quarantine flag once after installing:

```bash
xattr -cr /Applications/FlowClone.app
```

The app then launches normally. Long-term fix requires an Apple Developer certificate: add the six `APPLE_*` secrets (uncomment them in `.github/workflows/ci.yml`) and future releases will be signed and notarized.

### macOS: hotkey does nothing after an update/rebuild

Ad-hoc signatures change on every build, and macOS ties TCC permissions to the signature — so replacing the bundle revokes **Input Monitoring** (and Accessibility) silently. The dictation pipeline waits for both permissions before starting the hotkey watcher, so the symptom is "hotkey dead, no errors". Fix: System Settings → Privacy & Security → toggle FlowClone back on under **Input Monitoring** and **Accessibility**, then relaunch. The Hub's Settings page shows live status for both.

### macOS: pill doesn't appear while recording

The pill window is shown/hidden natively by the Rust core on every pipeline transition, its position is clamped into the visible monitor area on each show, and the webview additionally reconciles state via polling — a missed event or an off-screen saved position can no longer leave you recording blind. If it still misbehaves, check that `flowBarPos` in the app DB isn't pinned to a disconnected display.

### Windows: SmartScreen warning on first install

Unsigned installers trigger "Windows protected your PC". Click **More info → Run anyway**, or use the `.msi` variant. Injection into apps running as Administrator requires OpenFlow to run elevated too.

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
