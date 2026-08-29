# OpenFlow

OpenFlow is the project name. The installed application name is FlowClone.

FlowClone converts speech to text on macOS and Windows. FlowClone processes the recorded audio on the device.

By default, press `Right Shift` once to start a dictation session. Press `Right Shift` again to stop the session.

You can also select push-to-talk mode in Settings.

FlowClone uses Tauri 2, Rust, React, and TypeScript.

## Main functions

FlowClone has these main functions:

- FlowClone supports toggle mode and push-to-talk mode.
- Push-to-talk mode supports hands-free dictation. Press the hotkey two times quickly to start this mode.
- FlowClone stops a hands-free session after approximately 1.5 seconds without detected speech.
- You can pause, continue, stop, or cancel a dictation session.
- You can select the system microphone or a specified microphone.
- The application can make a sound when a dictation session starts or stops.
- The audio engine can reduce noise. The audio engine removes silence before transcription.
- A local Whisper model converts the audio to text. A build can also include Parakeet.
- A local llama.cpp model can remove filler words and correct the text.
- FlowClone can paste the raw text if the cleanup model has an error.
- Exact snippet matches expand without the cleanup model.
- Voice commands can open a website, do a search, copy text, or remove the last paste.
- The dictionary stores names, terms, and replacement rules.
- FlowClone can add frequent vocabulary corrections to the dictionary or to a review list.
- Cleanup can use the text before the cursor as context.
- Application styles can specify the cleanup instructions and the transcription language.
- You can select a style from the Flow Bar.
- FlowClone supports 18 specified languages and automatic language detection.
- The Flow Bar shows the microphone level and the pipeline state.
- The Flow Bar can show a live preview of completed speech phrases while you dictate.
- Live previews may skip a phrase when final transcription is active. The final
  transcription is the authoritative result.
- The system tray gives controls for the dictation session, the Hub, and the application.
- The Hub stores searchable transcript history. You can star, copy, delete, or paste a transcript again.
- The onboarding procedure checks permissions and helps you download the first transcription model.
- The active capture limit is six minutes. Paused time does not count.

## Data and network use

FlowClone stores the history, dictionary, snippets, styles, and settings in a local SQLite database.

FlowClone keeps each audio recording in memory. FlowClone does not store recorded audio in the database.

FlowClone does not send recorded audio to a network service. The application uses the network for model downloads and update checks.

## Architecture

The project has these main components:

| Component | Function |
| --- | --- |
| React user interface | Shows the Hub and the Flow Bar. |
| Tauri IPC | Connects the user interface to the Rust core. |
| `hotkey.rs` and `hotkey_tap.rs` | Detect global hotkey events. |
| `audio.rs` | Captures microphone input and makes 16-bit WAV data. |
| `pipeline.rs` | Controls recording, transcription, cleanup, commands, and paste operations. |
| `cloud/local_stt.rs` | Runs local Whisper transcription and selects the optional Parakeet engine. |
| `cloud/local_llm.rs` | Runs local cleanup in the `cleanup-engine` process. |
| `commands.rs` | Identifies and runs voice commands. |
| `inject/` | Sends paste and undo shortcuts on macOS and Windows. |
| `store.rs` | Stores application data in SQLite. |

## Dictation sequence

FlowClone uses this sequence for each dictation session:

1. The hotkey starts a dictation session.
2. FlowClone saves the target application and starts the selected microphone.
3. The audio engine converts the input to 16 kHz mono WAV data.
4. During recording, FlowClone can transcribe completed speech phrases for a live Flow Bar preview.
5. After recording stops, the selected local model converts the complete audio to the final text.
6. FlowClone expands an exact snippet match without the cleanup model.
7. If you enable cleanup, the local cleanup model corrects the text.
8. A recognized voice command runs instead of a paste operation.
9. FlowClone stages the text on the clipboard and sends the paste shortcut.
10. FlowClone restores the previous clipboard value if the clipboard did not change.

The live preview is for feedback only. The final transcription replaces the preview before FlowClone pastes text or stores history.

A worker thread does transcription, cleanup, command, and paste operations. The hotkey thread remains available while the worker thread operates.

Press `Esc` during recording or processing to cancel the result.

## Development setup

### Requirements

The development environment has these requirements:

- Node.js 22
- npm
- Rust stable
- Xcode Command Line Tools on macOS
- Microsoft C++ Build Tools on Windows
- CMake for the cleanup engine

Run these commands from the repository root:

```bash
npm install
npm run copy-engine
npm run tauri dev
```

The `copy-engine` command makes a required placeholder when no cleanup-engine binary is available.

To make a complete offline application bundle, run this command:

```bash
npm run build:offline
```

This command builds the cleanup engine, stages its binary, and then builds the application bundle.

## First use

Before you use FlowClone for the first time, do these steps:

1. Start FlowClone.
2. On macOS, grant Accessibility permission.
3. On macOS, grant Input Monitoring permission.
4. Grant Microphone permission.
5. On the onboarding model page, select a Whisper model.
6. Select **Download**.
7. Wait until the model status is **Ready**.
8. Complete the dictation test.

To use the default toggle mode, do these steps:

1. Put the text cursor in the target application.
2. Press `Right Shift`.
3. Speak.
4. Press `Right Shift` again.

FlowClone pastes the result at the text cursor.

## Model management

If a transcription model is not available, do these steps:

1. Open the Hub.
2. Open **Settings**.
3. Go to **Transcription**.
4. Select **Whisper**.
5. Select **Download** for the applicable model.
6. Wait until the model status is **Ready**.

A build with the `parakeet` feature shows the Parakeet control. Standard builds use Whisper.

Open **Settings**, and then go to **Cleanup** to download a cleanup model.

FlowClone verifies the downloaded file size. If a download is incomplete, select **Retry**.

The download has no total time limit. It can run longer than two minutes when
data continues to arrive. The download stops after 60 seconds with no data.

Cleanup has a 15-second limit. If cleanup does not finish, FlowClone pastes
the raw transcription.

## Platform information

### macOS

FlowClone requires Accessibility, Input Monitoring, and Microphone permissions.

FlowClone stages text on the clipboard and sends `Command+V` with CoreGraphics. FlowClone uses System Events if CoreGraphics cannot send the shortcut.

After 800 ms, FlowClone restores the previous clipboard value if the clipboard did not change.

The native event tap detects the global hotkey. The event tap starts after the required permissions are available.

macOS can remove permissions after an application bundle changes. Grant the permissions again after an update if the hotkey does not operate.

### Windows

Windows does not require Accessibility or Input Monitoring permission.

FlowClone stages text on the clipboard and sends `Ctrl+V` with `SendInput`.

UI Automation reads cursor context when the target control supports `TextPattern`.

If the target application runs as administrator, start FlowClone as administrator.

## Build and release process

Each push to `main` starts frontend and Rust tests and a rolling development build. CI builds macOS and Windows packages.

CI publishes the development packages in the `dev` release. FlowClone can show the new version in **Settings**.

Use a version tag to build a stable release:

```bash
git tag v0.4.0
git push origin v0.4.0
```

The release process requires these repository secrets:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

The updater public key is in `src-tauri/tauri.conf.json`.

## Validation

On a new checkout, run `npm run copy-engine` before the Rust commands.

Run these commands before you submit a change:

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Use these commands to validate the optional engines:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features parakeet
cargo clippy --manifest-path src-tauri/Cargo.toml --features parakeet -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --features engine --bins -- -D warnings
```

The `ci.yml` workflow runs the standard checks on macOS and Windows.

## Troubleshooting

### FlowClone shows "The transcription model is not downloaded"

Open **Settings**, and then go to **Transcription**. Select **Whisper**, and then select **Download** for a model.

If the model list does not appear, select **Retry**. Restart FlowClone if the list still does not appear.

### FlowClone shows a music label

FlowClone ignores complete sound labels such as `[Music]`, `[Silence]`, and
`[Noise]`. It does not paste or save these labels. Real speech such as “play
music” remains available.

### macOS shows the "FlowClone is damaged" message

Current macOS packages use an ad-hoc signature. Gatekeeper can prevent the first start.

After you put FlowClone in the Applications folder, run this command one time:

```bash
xattr -cr /Applications/FlowClone.app
```

### The macOS hotkey does not operate after an update

Do these steps:

1. Open **System Settings**.
2. Open **Privacy & Security**.
3. Open **Input Monitoring** and enable FlowClone.
4. Open **Accessibility** and enable FlowClone.
5. Restart FlowClone.

The Settings page shows the current state of both permissions.

### Windows SmartScreen shows a warning

The Windows package does not have a commercial code-signing certificate. SmartScreen can show a warning during the first installation.

In SmartScreen, select **More info**, and then select **Run anyway**.

### FlowClone does not detect speech

Open **Settings**, and then go to **Dictation**. Read the active microphone name below the microphone selector.

Select a different microphone if the active microphone is not correct. If FlowClone shows a fallback warning, connect the saved microphone or select an available microphone.

## Documentation language

This README uses [ASD-STE100 Issue 9](https://www.asd-ste100.org/assets/files/ASD-STE100_ISSUE9.pdf) as its language reference.

Software terms, file names, and command text are technical nouns or technical verbs.

## License

The MIT License applies to this project.
