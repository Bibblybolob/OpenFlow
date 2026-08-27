import { useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { api } from "../../lib/ipc";
import {
  ACCENTS,
  DEFAULT_PILL_STYLE,
  PILL_STYLE_KEY,
  accentOf,
  loadPillStyle,
  pillBackground,
  pillRadius,
  rgba,
  shade,
  type PillAccent,
  type PillShape,
  type PillStyle,
} from "../../lib/pillStyle";
import { LANGUAGES } from "../../lib/languages";


interface LocalModelInfo {
  id: string;
  label: string;
  approxMb: number;
  downloaded: boolean;
}

interface LocalParakeetStatus {
  id: string;
  available: boolean;
  downloaded: boolean;
}

interface DownloadError {
  model: string;
  message: string;
}

export default function Settings({
  onRerunSetup,
}: {
  onRerunSetup: () => void;
}) {
  const [flowbarPreset, setFlowbarPreset] = useState("bottom_center");
  const [pillStyle, setPillStyle] = useState<PillStyle>(DEFAULT_PILL_STYLE);
  const [localModels, setLocalModels] = useState<LocalModelInfo[]>([]);
  const [localModel, setLocalModel] = useState("base");
  const [localDownload, setLocalDownload] = useState<string | null>(null);
  const [localDownloadError, setLocalDownloadError] =
    useState<DownloadError | null>(null);
  const [localEngine, setLocalEngine] = useState<"whisper" | "parakeet">(
    "whisper",
  );
  const [parakeetStatus, setParakeetStatus] = useState<LocalParakeetStatus>({
    id: "parakeet-tdt-0.6b-v3",
    available: false,
    downloaded: false,
  });
  const [parakeetDownload, setParakeetDownload] = useState<string | null>(
    null,
  );
  const [parakeetDownloadError, setParakeetDownloadError] =
    useState<string | null>(null);
  const [cleanupEnabled, setCleanupEnabled] = useState(true);
  const [skipShort, setSkipShort] = useState(true);
  const [localLlms, setLocalLlms] = useState<LocalModelInfo[]>([]);
  const [localLlm, setLocalLlm] = useState("qwen3-4b");
  const [llmDownload, setLlmDownload] = useState<string | null>(null);
  const [llmDownloadError, setLlmDownloadError] =
    useState<DownloadError | null>(null);
  const [language, setLanguage] = useState("auto");
  const [hotkey, setHotkey] = useState<string[]>(["Right Shift"]);
  const [hotkeyOptions, setHotkeyOptions] = useState<string[]>([
    "F1", "CapsLock", "Right Shift",
  ]);
  const [watcherStatus, setWatcherStatus] = useState("waiting-permissions");
  const [mics, setMics] = useState<string[]>([]);
  const [mic, setMic] = useState<string>("");
  const [soundEffects, setSoundEffects] = useState(true);
  const [noiseSuppression, setNoiseSuppression] = useState(true);
  const [voiceSensitivity, setVoiceSensitivity] = useState("medium");
  const [hotkeyMode, setHotkeyMode] = useState("toggle");
  const [autostart, setAutostart] = useState(false);
  const [commandMode, setCommandMode] = useState(true);
  const [accessibility, setAccessibility] = useState<boolean | null>(null);
  const [inputMonitoring, setInputMonitoring] = useState<boolean | null>(null);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const [appVersion, setAppVersion] = useState<string>("");
  const [availableVersion, setAvailableVersion] = useState<string | null>(
    null,
  );

  // Poll the rolling dev channel on mount: a newer main build shows the
  // "Update now" pill without the user hunting for the button.
  useEffect(() => {
    let cancelled = false;
    api
      .appVersion()
      .then((v) => !cancelled && setAppVersion(v))
      .catch(() => {});
    api
      .checkForUpdate()
      .then((v) => !cancelled && setAvailableVersion(v))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);


  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const unlisten = listen<ModelProgressPayload>(
      "local-model-progress",
      (event) => {
        const payload = event.payload;
        if (payload.type === "done") {
          setLocalDownload(null);
          setLocalDownloadError(null);
          api.localModelStatus().then(setLocalModels).catch(() => {});
          return;
        }
        if (payload.type === "error") {
          setLocalDownload(null);
          setLocalDownloadError({
            model: payload.model,
            message: payload.message ?? "The model download failed. Try again.",
          });
          return;
        }
        setLocalDownloadError(null);
        setLocalDownload(
          `${payload.model}: ${payload.downloadedMb} MB${payload.totalMb ? `/${payload.totalMb} MB` : ""}`,
        );
      },
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<ModelProgressPayload>(
      "local-parakeet-progress",
      (event) => {
        const payload = event.payload;
        if (payload.type === "done") {
          setParakeetDownload(null);
          setParakeetDownloadError(null);
          api.localParakeetStatus().then(setParakeetStatus).catch(() => {});
          return;
        }
        if (payload.type === "error") {
          setParakeetDownload(null);
          setParakeetDownloadError(
            payload.message ?? "The Parakeet download failed. Try again.",
          );
          return;
        }
        setParakeetDownloadError(null);
        setParakeetDownload(
          `${payload.downloadedMb} MB${payload.totalMb ? `/${payload.totalMb} MB` : ""}`,
        );
      },
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<{ status: string; detail?: string }>(
      "hotkey-status",
      (event) => {
        setWatcherStatus(event.payload.status);
        refresh();
      },
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function refresh() {
    const lang = (await api.getSetting<string>("language")) ?? "auto";
    const hk = await api.getHotkey().catch(() => ["Right Shift"]);
    const hkOptions = await api
      .hotkeyOptions()
      .catch(() => ["F1", "CapsLock", "Right Shift"]);
    const ws = await api.hotkeyWatcherStatus().catch(() => "waiting-permissions");
    const micList = await api.listMics().catch(() => [] as string[]);
    const micPref = await api.getSetting<string>("micDevice");
    const se = await api.getSetting<boolean>("soundEffects");
    const ns = await api.getSetting<boolean>("noiseSuppression");
    const vs = await api.getSetting<string>("voiceSensitivity");
    const hmode = await api.getSetting<string>("hotkeyMode");
    const as = await api.autostartStatus().catch(() => false);
    const cm = await api.getSetting<boolean>("commandMode");
    const ce = await api.getSetting<boolean>("cleanupEnabled");
    const ss = await api.getSetting<boolean>("cleanupSkipShort");
    const le = await api.getSetting<string>("sttLocalEngine");
    const lm = await api.getSetting<string>("sttLocalModel");
    const lms = await api.localModelStatus().catch(() => [] as LocalModelInfo[]);
    const ps = await api.localParakeetStatus().catch(
      () =>
        ({
          id: "parakeet-tdt-0.6b-v3",
          available: false,
          downloaded: false,
        }) as LocalParakeetStatus,
    );
    const llmId = await api.getSetting<string>("llmLocalModel");
    const llms = await api.localLlmStatus().catch(() => [] as LocalModelInfo[]);
    const style = await loadPillStyle();

    setLanguage(lang ?? "auto");
    setHotkey(hk.length ? hk : ["Right Shift"]);
    setHotkeyOptions(hkOptions.length ? hkOptions : ["F1", "CapsLock", "Right Shift"]);
    setWatcherStatus(ws);
    setMics(micList);
    setMic(micPref ?? "");
    setSoundEffects(se ?? true);
    setNoiseSuppression(ns ?? true);
    setVoiceSensitivity(vs ?? "medium");
    setHotkeyMode(hmode ?? "toggle");
    setAutostart(as);
    setCommandMode(cm ?? true);
    setCleanupEnabled(ce ?? true);
    setSkipShort(ss ?? true);
    setLocalEngine(le === "parakeet" ? "parakeet" : "whisper");
    setLocalModel(lm ?? "base");
    setLocalModels(lms);
    setParakeetStatus(ps);
    setLocalLlm(llmId ?? "qwen3-4b");
    setLocalLlms(llms);
    setPillStyle(style);
    setAccessibility(await invokeAccessibility());
    setInputMonitoring(await api.inputMonitoringStatus().catch(() => false));
  }

  useEffect(() => {
    const unlisten = listen<ModelProgressPayload>(
      "local-llm-progress",
      (event) => {
        const payload = event.payload;
        if (payload.type === "done") {
          setLlmDownload(null);
          setLlmDownloadError(null);
          api.localLlmStatus().then(setLocalLlms).catch(() => {});
          return;
        }
        if (payload.type === "error") {
          setLlmDownload(null);
          setLlmDownloadError({
            model: payload.model,
            message: payload.message ?? "The cleanup model download failed. Try again.",
          });
          return;
        }
        setLlmDownloadError(null);
        setLlmDownload(
          `${payload.model}: ${payload.downloadedMb} MB${payload.totalMb ? `/${payload.totalMb} MB` : ""}`,
        );
      },
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  async function changeLocalLlm(id: string) {
    setLocalLlm(id);
    try {
      await api.setLocalLlm(id);
      const info = localLlms.find((m) => m.id === id);
      if (info && !info.downloaded) {
        await downloadLocalLlm(id);
      }
    } catch (e) {
      console.error(e);
      setLlmDownloadError({ model: id, message: readableError(e) });
    }
  }

  async function changeFlowbarPreset(preset: string) {
    setFlowbarPreset(preset);
    try {
      await api.setFlowbarPreset(preset);
    } catch (e) {
      console.error(e);
    }
  }

  async function changePillStyle(patch: Partial<PillStyle>) {
    const next = { ...pillStyle, ...patch };
    setPillStyle(next);
    try {
      await api.setSetting(PILL_STYLE_KEY, next);
      await emit("flowbar-style-changed");
    } catch (e) {
      console.error(e);
    }
  }

  async function changeHotkey(name: string) {
    setHotkey([name]);
    try {
      const applied = await api.setHotkey([name]);
      setHotkey(applied);
    } catch (e) {
      console.error(e);
    }
  }

  async function changeLocalModel(id: string) {
    setLocalModel(id);
    try {
      await api.setLocalModel(id);
      const info = localModels.find((m) => m.id === id);
      if (info && !info.downloaded) {
        await downloadLocalModel(id);
      }
    } catch (e) {
      console.error(e);
      setLocalDownloadError({ model: id, message: readableError(e) });
    }
  }

  async function downloadLocalModel(id: string) {
    if (localDownload !== null) return;
    setLocalDownloadError(null);
    setLocalDownload(`${id}: starting…`);
    try {
      await api.downloadLocalModel(id);
    } catch (e) {
      console.error(e);
      setLocalDownload(null);
      setLocalDownloadError({ model: id, message: readableError(e) });
    }
  }

  async function downloadLocalLlm(id: string) {
    if (llmDownload !== null) return;
    setLlmDownloadError(null);
    setLlmDownload(`${id}: starting…`);
    try {
      await api.downloadLocalLlm(id);
    } catch (e) {
      console.error(e);
      setLlmDownload(null);
      setLlmDownloadError({ model: id, message: readableError(e) });
    }
  }

  async function downloadParakeet() {
    if (parakeetDownload !== null || !parakeetStatus.available) return;
    setParakeetDownloadError(null);
    setParakeetDownload("starting…");
    try {
      await api.downloadLocalParakeet();
    } catch (e) {
      console.error(e);
      setParakeetDownload(null);
      setParakeetDownloadError(readableError(e));
    }
  }

  async function changeLocalEngine(engine: "whisper" | "parakeet") {
    if (engine === "parakeet" && !parakeetStatus.available) return;
    try {
      if (engine === "parakeet" && !parakeetStatus.downloaded) {
        await downloadParakeet();
      }
      await api.setSetting("sttLocalEngine", engine);
      setLocalEngine(engine);
    } catch (e) {
      console.error(e);
      setParakeetDownloadError(readableError(e));
    }
  }

  async function toggleCleanupEnabled() {
    const next = !cleanupEnabled;
    setCleanupEnabled(next);
    try {
      await api.setSetting("cleanupEnabled", next);
    } catch (e) {
      console.error(e);
      setCleanupEnabled(!next);
    }
  }

  async function toggleSkipShort() {
    const next = !skipShort;
    setSkipShort(next);
    try {
      await api.setSetting("cleanupSkipShort", next);
    } catch (e) {
      console.error(e);
      setSkipShort(!next);
    }
  }

  async function changeLanguage(code: string) {
    setLanguage(code);
    await api.setSetting("language", code === "auto" ? "" : code);
  }

  async function changeMic(name: string) {
    setMic(name);
    try {
      await api.setMicDevice(name || null);
    } catch (e) {
      console.error(e);
    }
  }

  async function toggleNoiseSuppression() {
    const next = !noiseSuppression;
    setNoiseSuppression(next);
    await api.setSetting("noiseSuppression", next);
  }

  async function changeHotkeyMode(v: string) {
    setHotkeyMode(v);
    await api.setSetting("hotkeyMode", v);
  }

  async function changeVoiceSensitivity(v: string) {
    setVoiceSensitivity(v);
    await api.setSetting("voiceSensitivity", v);
  }

  async function toggleSoundEffects() {
    const next = !soundEffects;
    setSoundEffects(next);
    try {
      await api.setSetting("soundEffects", next);
    } catch (e) {
      console.error(e);
      setSoundEffects(!next);
    }
  }

  async function toggleAutostart() {
    const next = !autostart;
    setAutostart(next);
    try {
      await api.setAutostart(next);
    } catch (e) {
      console.error(e);
      setAutostart(!next);
    }
  }

  async function toggleCommandMode() {
    const next = !commandMode;
    setCommandMode(next);
    try {
      await api.setSetting("commandMode", next);
    } catch (e) {
      console.error(e);
      setCommandMode(!next);
    }
  }

  async function checkForUpdate() {
    setUpdateStatus("Checking…");
    try {
      const version = await api.checkForUpdate();
      if (version) {
        setAvailableVersion(version);
        setUpdateStatus(`Version ${version} available — installing…`);
        await api.installUpdate();
        setUpdateStatus("Installed. Restarting…");
      } else {
        setAvailableVersion(null);
        setUpdateStatus("You're up to date.");
      }
    } catch (e) {
      console.error(e);
      setUpdateStatus(String(e).replace(/^.*failed: /, "Check failed: "));
    }
  }

  async function updateNow() {
    setUpdateStatus(`Updating to ${availableVersion}…`);
    try {
      await api.installUpdate();
      setUpdateStatus("Installed. Restarting…");
    } catch (e) {
      console.error(e);
      setUpdateStatus(String(e).replace(/^.*failed: /, "Update failed: "));
    }
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Settings</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Transcription, cleanup LLM, permissions, and shortcuts.
        </p>
      </div>

      <button
        onClick={onRerunSetup}
        className="self-start rounded-lg border border-white/10 px-4 py-2 text-xs text-neutral-400 transition hover:bg-white/5"
      >
        Run setup wizard again
      </button>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          About
        </h2>
        <div className="flex items-center justify-between rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <span className="text-sm text-neutral-300">
            Version{" "}
            {appVersion || (import.meta.env.PACKAGE_VERSION ?? "0.1.0")}
          </span>
          <div className="flex items-center gap-3">
            {updateStatus && (
              <span className="text-xs text-neutral-500">{updateStatus}</span>
            )}
            {availableVersion && !updateStatus && (
              <button
                onClick={updateNow}
                className="rounded-md bg-indigo-500/90 px-3 py-1 text-xs font-medium text-white transition hover:bg-indigo-400"
              >
                Update now → {availableVersion}
              </button>
            )}
            <button
              onClick={checkForUpdate}
              className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 transition hover:bg-white/5"
            >
              Check for updates
            </button>
          </div>
        </div>
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Dictation
        </h2>
        <div className="flex gap-2">
          <SelectRow
            label="Hotkey"
            value={hotkey.join(" + ")}
            options={hotkeyOptions.map((k) => ({ value: k, label: k }))}
            onChange={(v) => changeHotkey(v)}
          />
        <SelectRow
          label="Activation"
          value={hotkeyMode}
          options={[
            { value: "toggle", label: "Toggle — press to start, press to stop" },
            { value: "push_to_talk", label: "Push-to-talk — hold the key" },
          ]}
          onChange={changeHotkeyMode}
        />
        <SelectRow
            label="Language"
            value={language || "auto"}
            options={LANGUAGES.map((l) => ({ value: l.code, label: l.label }))}
            onChange={changeLanguage}
          />
        </div>
        {hotkey.includes("F5") && (
          <p className="text-xs text-amber-500">
            F5 is the mic/dictation key on many Mac keyboards — macOS sends it
            as a special key that FlowClone cannot see. Pick another key.
          </p>
        )}
        <SelectRow
          label="Microphone"
          value={mic}
          options={[
            { value: "", label: "System default" },
            ...mics.map((m) => ({ value: m, label: m })),
          ]}
          onChange={changeMic}
        />
        <ToggleRow
          label="Noise suppression"
          hint="RNNoise-style model removes keyboard, fan and room noise before transcription"
          checked={noiseSuppression}
          onChange={toggleNoiseSuppression}
        />
        <SelectRow
          label="Voice sensitivity"
          value={voiceSensitivity}
          options={[
            { value: "low", label: "Low — noisy rooms" },
            { value: "medium", label: "Medium (recommended)" },
            { value: "high", label: "High — only clear speech" },
          ]}
          onChange={changeVoiceSensitivity}
        />
        <p className="text-xs text-neutral-600">
          Recordings where no voice clears the noise floor are discarded
          instead of transcribed, preventing phantom text from silence.
        </p>
        <ToggleRow
          label="Launch at login"
          hint="Start FlowClone automatically when you sign in"
          checked={autostart}
          onChange={toggleAutostart}
        />
        <ToggleRow
          label="Start & stop chimes"
          hint="Subtle audio feedback when dictation starts and stops"
          checked={soundEffects}
          onChange={toggleSoundEffects}
        />
        <ToggleRow
          label="Voice commands"
          hint='Say things like "open youtube", "search rust async", or "copy …" to act instead of typing'
          checked={commandMode}
          onChange={toggleCommandMode}
        />
        <p className="text-xs text-neutral-600">
          Hold the hotkey to dictate. Quick double-tap switches to hands-free
          mode; tap again or press Esc to stop.
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Transcription
        </h2>
        <div className="flex items-center gap-3">
          <div className="flex rounded-lg border border-white/10 bg-white/[0.04] p-1">
            <button
              type="button"
              onClick={() => changeLocalEngine("whisper")}
              className={`rounded-md px-3 py-1.5 text-sm ${
                localEngine === "whisper"
                  ? "bg-white/10 text-white"
                  : "text-neutral-500 hover:text-neutral-300"
              }`}
            >
              Whisper
            </button>
            <button
              type="button"
              onClick={() => changeLocalEngine("parakeet")}
              disabled={!parakeetStatus.available}
              className={`rounded-md px-3 py-1.5 text-sm ${
                localEngine === "parakeet"
                  ? "bg-white/10 text-white"
                  : "text-neutral-500 hover:text-neutral-300"
              } disabled:cursor-not-allowed disabled:opacity-40`}
            >
              Parakeet
            </button>
          </div>
          <p className="flex-1 self-center text-xs text-neutral-600">
            {localEngine === "whisper"
              ? "Runs entirely offline — choose a Whisper model below."
              : "Runs entirely offline with Parakeet TDT; first use downloads its model bundle."}
          </p>
        </div>
        {localEngine === "whisper" ? (
          <div className="flex flex-col gap-2 rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
            {localModels.map((m) => (
              <div key={m.id} className="flex flex-col gap-2">
                <div className="flex items-center justify-between gap-4">
                  <label className="flex flex-1 items-center gap-3 text-sm">
                    <input
                      type="radio"
                      name="localModel"
                      checked={localModel === m.id}
                      onChange={() => changeLocalModel(m.id)}
                      className="accent-indigo-400"
                    />
                    <span className="text-neutral-300">{m.label}</span>
                    {!m.downloaded && (
                      <span className="text-xs text-neutral-600">
                        ~{m.approxMb} MB
                      </span>
                    )}
                  </label>
                  <div className="flex shrink-0 items-center gap-3">
                    <span
                      className={`text-xs ${
                        m.downloaded ? "text-emerald-400" : "text-neutral-500"
                      }`}
                    >
                      {localDownload?.startsWith(`${m.id}:`)
                        ? localDownload.split(": ", 2)[1] ?? "Downloading…"
                        : m.downloaded
                          ? "Ready"
                          : "Not downloaded"}
                    </span>
                    {!m.downloaded && (
                      <button
                        type="button"
                        onClick={() => downloadLocalModel(m.id)}
                        disabled={localDownload !== null}
                        className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/[0.06] disabled:cursor-wait disabled:opacity-50"
                      >
                        {localDownload?.startsWith(`${m.id}:`)
                          ? "Downloading…"
                          : "Download"}
                      </button>
                    )}
                  </div>
                </div>
                {localDownloadError?.model === m.id && (
                  <div className="flex items-center justify-between gap-3 rounded-md bg-red-500/10 px-3 py-2 text-xs text-red-300">
                    <span>{localDownloadError.message}</span>
                    <button
                      type="button"
                      onClick={() => downloadLocalModel(m.id)}
                      className="shrink-0 text-red-200 underline underline-offset-2 hover:text-white"
                    >
                      Retry
                    </button>
                  </div>
                )}
              </div>
            ))}
          </div>
        ) : (
          <div className="flex flex-col gap-1 rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
            <div className="flex items-center justify-between gap-4 text-sm">
              <span className="text-neutral-300">Parakeet TDT 0.6B v3</span>
              <div className="flex items-center gap-3">
                <span
                  className={
                    parakeetStatus.downloaded
                      ? "text-xs text-emerald-400"
                      : "text-xs text-neutral-500"
                  }
                >
                  {parakeetDownload ??
                    (parakeetStatus.downloaded ? "Ready" : "Not downloaded")}
                </span>
                {!parakeetStatus.downloaded && (
                  <button
                    type="button"
                    onClick={downloadParakeet}
                    disabled={parakeetDownload !== null}
                    className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/[0.06] disabled:cursor-wait disabled:opacity-50"
                  >
                    {parakeetDownload ? "Downloading…" : "Download"}
                  </button>
                )}
              </div>
            </div>
            {parakeetDownloadError && (
              <div className="flex items-center justify-between gap-3 rounded-md bg-red-500/10 px-3 py-2 text-xs text-red-300">
                <span>{parakeetDownloadError}</span>
                <button
                  type="button"
                  onClick={downloadParakeet}
                  className="shrink-0 text-red-200 underline underline-offset-2 hover:text-white"
                >
                  Retry
                </button>
              </div>
            )}
            <p className="text-xs text-neutral-600">
              ~300 MB download. Offline batch recognition; Whisper-only prompt
              and language hints are not applied.
            </p>
          </div>
        )}
        {!parakeetStatus.available && (
          <p className="text-xs text-neutral-600">
            Parakeet is unavailable in this app build; Whisper remains the
            default local engine.
          </p>
        )}
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Flow Bar
        </h2>
        <SelectRow
          label="Position"
          value={flowbarPreset}
          options={[
            { value: "top_left", label: "Top left" },
            { value: "top_center", label: "Top center" },
            { value: "top_right", label: "Top right" },
            { value: "bottom_left", label: "Bottom left" },
            { value: "bottom_center", label: "Bottom center" },
            { value: "bottom_right", label: "Bottom right" },
          ]}
          onChange={changeFlowbarPreset}
        />
        <p className="text-xs text-neutral-600">
          You can also grab the pill and drag it anywhere — its spot is
          remembered across restarts.
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Pill appearance
        </h2>
        <div className="flex items-center justify-center rounded-xl border border-white/5 bg-white/[0.02] py-8">
          <PillPreview style={pillStyle} />
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <p className="mb-2 text-sm text-neutral-300">Shape</p>
          <div className="flex gap-2">
            {(
              [
                ["pill", "Pill"],
                ["rounded", "Rounded"],
                ["square", "Square"],
              ] as [PillShape, string][]
            ).map(([value, label]) => (
              <button
                key={value}
                onClick={() => changePillStyle({ shape: value })}
                className={`flex-1 rounded-lg border px-3 py-2 text-xs transition ${
                  pillStyle.shape === value
                    ? "border-indigo-400/60 bg-indigo-500/15 text-white"
                    : "border-white/10 text-neutral-400 hover:bg-white/5"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <p className="mb-2 text-sm text-neutral-300">Accent</p>
          <div className="flex gap-2">
            {(Object.keys(ACCENTS) as PillAccent[]).map((key) => (
              <button
                key={key}
                title={ACCENTS[key].label}
                onClick={() => changePillStyle({ accent: key })}
                className={`h-7 w-7 rounded-full border-2 transition ${
                  pillStyle.accent === key
                    ? "scale-110 border-white"
                    : "border-transparent hover:scale-105"
                }`}
                style={{ backgroundColor: ACCENTS[key].swatch }}
              />
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <div className="mb-2 flex items-center justify-between">
            <p className="text-sm text-neutral-300">Opacity</p>
            <span className="text-xs text-neutral-500">{pillStyle.opacity}%</span>
          </div>
          <input
            type="range"
            min={40}
            max={100}
            step={1}
            value={pillStyle.opacity}
            onChange={(e) =>
              setPillStyle({ ...pillStyle, opacity: Number(e.target.value) })
            }
            onMouseUp={(e) =>
              changePillStyle({ opacity: Number((e.target as HTMLInputElement).value) })
            }
            onTouchEnd={(e) =>
              changePillStyle({ opacity: Number((e.target as HTMLInputElement).value) })
            }
            onKeyUp={(e) =>
              changePillStyle({ opacity: Number((e.target as HTMLInputElement).value) })
            }
            className="w-full accent-indigo-400"
          />
        </div>
        <ToggleRow
          label="Animations"
          hint="Pop-in motion, glow while recording, button feedback"
          checked={pillStyle.animations}
          onChange={() => changePillStyle({ animations: !pillStyle.animations })}
        />
        <ToggleRow
          label="Hide when idle"
          hint='The pill only appears while dictating; bring it back with the hotkey'
          checked={pillStyle.autoHide}
          onChange={() => changePillStyle({ autoHide: !pillStyle.autoHide })}
        />
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Cleanup
        </h2>
        <ToggleRow
          label="AI cleanup"
          hint="Removes fillers and fixes punctuation — turn off to paste raw transcription with minimum latency"
          checked={cleanupEnabled}
          onChange={toggleCleanupEnabled}
        />
        <ToggleRow
          label="Fast path for short dictations"
          hint="Paste raw text without LLM cleanup for utterances under ~120 characters — saves the biggest latency chunk"
          checked={skipShort}
          onChange={toggleSkipShort}
        />
        <div className={`flex gap-2 ${cleanupEnabled ? "" : "pointer-events-none opacity-40"}`}>
          <div className="w-48 rounded-lg border border-white/10 bg-white/[0.04] px-3 py-2 text-sm text-neutral-300">
            On-device (offline)
          </div>
        </div>
        <div
            className={`flex flex-col gap-2 rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3 ${
              cleanupEnabled ? "" : "pointer-events-none opacity-40"
            }`}
          >
            {localLlms.map((m) => (
              <div key={m.id} className="flex flex-col gap-2">
                <div className="flex items-center justify-between gap-4">
                  <label className="flex flex-1 items-center gap-3 text-sm">
                    <input
                      type="radio"
                      name="localLlm"
                      checked={localLlm === m.id}
                      onChange={() => changeLocalLlm(m.id)}
                      className="accent-indigo-400"
                    />
                    <span className="text-neutral-300">{m.label}</span>
                    {!m.downloaded && (
                      <span className="text-xs text-neutral-600">
                        ~{m.approxMb} MB
                      </span>
                    )}
                  </label>
                  <span
                    className={`shrink-0 text-xs ${
                      m.downloaded ? "text-emerald-400" : "text-neutral-500"
                    }`}
                  >
                    {llmDownload?.startsWith(`${m.id}:`)
                      ? llmDownload.split(": ", 2)[1] ?? "Downloading…"
                      : m.downloaded
                        ? "Ready"
                        : "Not downloaded"}
                  </span>
                  {!m.downloaded && (
                    <button
                      type="button"
                      onClick={() => downloadLocalLlm(m.id)}
                      disabled={llmDownload !== null}
                      className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/[0.06] disabled:cursor-wait disabled:opacity-50"
                    >
                      {llmDownload?.startsWith(`${m.id}:`)
                        ? "Downloading…"
                        : "Download"}
                    </button>
                  )}
                </div>
                {llmDownloadError?.model === m.id && (
                  <div className="flex items-center justify-between gap-3 rounded-md bg-red-500/10 px-3 py-2 text-xs text-red-300">
                    <span>{llmDownloadError.message}</span>
                    <button
                      type="button"
                      onClick={() => downloadLocalLlm(m.id)}
                      className="shrink-0 text-red-200 underline underline-offset-2 hover:text-white"
                    >
                      Retry
                    </button>
                  </div>
                )}
              </div>
            ))}
            <p className="text-xs text-neutral-600">
              Runs entirely offline via llama.cpp (Metal). First use downloads
              the model.
            </p>
          </div>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Permissions
        </h2>
        <div className="overflow-hidden rounded-xl border border-white/5">
          <WatcherRow status={watcherStatus} />
          <KeySightRow />
          <PermRow label="Microphone" ok={true} hint="Granted on first use" />
          <PermRow
            label="Input Monitoring (global hotkey)"
            ok={inputMonitoring === true}
            hint={
              inputMonitoring === false
                ? "Required to detect the hotkey in any app — without it no hotkey can fire"
                : "Needed to read the hotkey while other apps are focused"
            }
            action={
              inputMonitoring === false ? (
                <button
                  onClick={openInputMonitoringSettings}
                  className="rounded-md bg-indigo-500/20 px-2 py-1 text-xs text-indigo-300 transition hover:bg-indigo-500/30"
                >
                  Open System Settings
                </button>
              ) : null
            }
          />
          <PermRow
            label="Accessibility (typing into apps)"
            ok={accessibility === true}
            hint={
              accessibility === false
                ? "Required to paste text and detect the frontmost app"
                : "Needed to paste at your cursor and match app styles"
            }
            action={
              accessibility === false ? (
                <button
                  onClick={openAccessibilitySettings}
                  className="rounded-md bg-indigo-500/20 px-2 py-1 text-xs text-indigo-300 transition hover:bg-indigo-500/30"
                >
                  Open System Settings
                </button>
              ) : null
            }
          />
        </div>
      </section>
    </div>
  );
}

function SelectRow({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (v: string) => void;
}) {
  return (
    <label className="flex flex-1 items-center gap-3 rounded-lg border border-white/5 bg-white/[0.03] px-4 py-2.5">
      <span className="w-16 shrink-0 text-xs text-neutral-500">{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="min-w-0 flex-1 bg-transparent text-sm text-neutral-200 outline-none [&>option]:bg-[#1a1a20]"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function ToggleRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <div className="flex items-center justify-between rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
      <div>
        <p className="text-sm text-neutral-300">{label}</p>
        <p className="text-[11px] text-neutral-600">{hint}</p>
      </div>
      <button
        role="switch"
        aria-checked={checked}
        onClick={onChange}
        className={`relative h-6 w-11 shrink-0 rounded-full transition-colors ${
          checked ? "bg-indigo-500" : "bg-white/10"
        }`}
      >
        <span
          className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${
            checked ? "left-[22px]" : "left-0.5"
          }`}
        />
      </button>
    </div>
  );
}


const KEY_SEEN_POLL_MS = 500;

function KeySightRow() {
  const [seen, setSeen] = useState<{ name: string; down: boolean; agoMs: number }[]>(
    [],
  );
  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const events = await api.hotkeyLastSeen();
        if (alive) setSeen(events);
      } catch {
        // backend may be starting up; keep last known
      }
    };
    tick();
    const id = setInterval(tick, KEY_SEEN_POLL_MS);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);
  const latest = seen[seen.length - 1];
  const trail = seen
    .slice(-5)
    .map((e) => `${e.name} ${e.down ? "\u2193" : "\u2191"}`)
    .join("   ");
  return (
    <div className="flex items-center justify-between bg-white/[0.03] px-4 py-3 not-last:border-b not-last:border-white/5">
      <div>
        <span className="text-sm text-neutral-300">Key diagnostics</span>
        <p className="mt-0.5 font-mono text-xs text-neutral-500">
          {latest
            ? `${trail}   ·   latest ${(latest.agoMs / 1000).toFixed(1)}s ago`
            : "press any key — what the backend sees appears here"}
        </p>
      </div>
      <span
        className={`text-xs ${
          seen.length ? "text-emerald-400" : "text-neutral-600"
        }`}
      >
        {seen.length ? "Seeing keys" : "Idle"}
      </span>
    </div>
  );
}

function WatcherRow({ status }: { status: string }) {
  const [prefix, detail] = status.split(":", 2);
  const ok = prefix === "ready";
  const label =
    prefix === "ready"
      ? "Hotkey backend"
      : prefix === "waiting-accessibility"
        ? "Hotkey backend — needs Accessibility"
        : prefix === "waiting-input-monitoring"
          ? "Hotkey backend — needs Input Monitoring"
          : prefix === "waiting-permissions"
            ? "Hotkey backend — waiting for permissions"
            : "Hotkey backend unavailable";
  const openRelevant = () =>
    (prefix === "waiting-accessibility"
      ? api.openAccessibilitySettings()
      : openInputMonitoringSettings()
    ).catch(() => {});
  return (
    <div className="flex items-center justify-between bg-white/[0.03] px-4 py-3 not-last:border-b not-last:border-white/5">
      <div>
        <span className="text-sm text-neutral-300">{label}</span>
        {detail && <p className="text-xs text-neutral-600">{detail}</p>}
        {!ok && prefix !== "unavailable" && (
          <p className="text-xs text-neutral-600">
            Without it the hotkey cannot fire in any app
          </p>
        )}
      </div>
      <div className="flex items-center gap-2">
        {!ok && (
          <button
            onClick={openRelevant}
            className="rounded-md bg-indigo-500/20 px-2 py-1 text-xs text-indigo-300 transition hover:bg-indigo-500/30"
          >
            Open System Settings
          </button>
        )}
        <span className={`text-xs ${ok ? "text-emerald-400" : "text-amber-400"}`}>
          {ok ? "Active" : "Check"}
        </span>
      </div>
    </div>
  );
}

function PermRow({
  label,
  ok,
  hint,
  action,
}: {
  label: string;
  ok: boolean;
  hint: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between bg-white/[0.03] px-4 py-3 not-last:border-b not-last:border-white/5">
      <div>
        <span className="text-sm text-neutral-300">{label}</span>
        {!ok && <p className="text-xs text-neutral-600">{hint}</p>}
      </div>
      {action ?? (
        <span
          className={`text-xs ${ok ? "text-emerald-400" : "text-amber-400"}`}
        >
          {ok ? "Granted" : "Check"}
        </span>
      )}
    </div>
  );
}

function PillPreview({ style }: { style: PillStyle }) {
  const accent = accentOf(style);
  const radius = pillRadius(style.shape);
  return (
    <div
      className={`flex items-center gap-3 border px-4 py-2 shadow-2xl ${radius}`}
      style={{
        background: pillBackground("active", style.opacity),
        borderColor: rgba(accent.border, 0.55),
      }}
    >
      <span
        className="flex h-9 w-9 items-center justify-center rounded-full text-white"
        style={{
          background: `linear-gradient(135deg, ${accent.base}, ${shade(accent.base, -0.4)})`,
        }}
      >
        <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
          <path d="M12 15a3.5 3.5 0 0 0 3.5-3.5V6a3.5 3.5 0 1 0-7 0v5.5A3.5 3.5 0 0 0 12 15Z" />
          <path d="M18.5 11.5a.75.75 0 0 0-1.5 0 5 5 0 0 1-10 0 .75.75 0 0 0-1.5 0 6.5 6.5 0 0 0 5.75 6.46V21h-2.25a.75.75 0 0 0 0 1.5h6a.75.75 0 0 0 0-1.5H12.75v-3.04a6.5 6.5 0 0 0 5.75-6.46Z" />
        </svg>
      </span>
      <div className="flex h-8 w-24 items-center justify-center gap-1">
        {[40, 65, 100, 78, 52].map((h, i) => (
          <span
            key={i}
            className="w-1 rounded-full"
            style={{ height: `${h}%`, backgroundColor: accent.soft }}
          />
        ))}
      </div>
      <span
        className="h-2 w-2 rounded-full"
        style={{ backgroundColor: rgba(accent.base, 0.85) }}
      />
    </div>
  );
}


async function invokeAccessibility(): Promise<boolean> {
  try {
    return await api.accessibilityStatus();
  } catch {
    return false;
  }
}

async function openAccessibilitySettings() {
  try {
    await api.openAccessibilitySettings();
  } catch {
    // best effort only
  }
}

async function openInputMonitoringSettings() {
  try {
    await api.openInputMonitoringSettings();
  } catch {
    // best effort only
  }
}

interface ModelProgressPayload {
  type?: string;
  model: string;
  downloadedMb: number;
  totalMb: number;
  message?: string;
}

function readableError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "The download failed. Try again.";
}
