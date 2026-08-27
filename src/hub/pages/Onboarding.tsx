import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { motion } from "framer-motion";
import { api } from "../../lib/ipc";
import { usePipelineState } from "../usePipelineState";

type Step = "welcome" | "permissions" | "model" | "hotkey" | "done";

interface CheckState {
  status: "pending" | "checking" | "ok" | "failed";
  message?: string;
}

interface LocalModelInfo {
  id: string;
  label: string;
  approxMb: number;
  downloaded: boolean;
}

interface ModelProgressPayload {
  type?: string;
  model: string;
  downloadedMb: number;
  totalMb: number;
  message?: string;
}

export default function Onboarding({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [accessibility, setAccessibility] = useState<CheckState>({ status: "pending" });
  const [inputMonitoring, setInputMonitoring] = useState<CheckState>({ status: "pending" });
  const [mic, setMic] = useState<CheckState>({ status: "pending" });
  const [hotkey, setHotkey] = useState<string[]>([]);
  const [localModels, setLocalModels] = useState<LocalModelInfo[]>([]);
  const [selectedModel, setSelectedModel] = useState("base");
  const [modelDownload, setModelDownload] = useState<string | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
  const { lastTranscriptId } = usePipelineState();

  useEffect(() => {
    api.getHotkey().then(setHotkey).catch(() => setHotkey(["Right Shift"]));
    api.setFlowbarVisible(false);
  }, []);

  const runChecks = useCallback(async () => {
    setAccessibility({ status: "checking" });
    setInputMonitoring({ status: "checking" });
    const [ax, input] = await Promise.all([
      api.accessibilityStatus().catch(() => false),
      api.inputMonitoringStatus().catch(() => false),
    ]);
    setAccessibility(
      ax
        ? { status: "ok" }
        : { status: "failed", message: "Grant Accessibility in System Settings, then re-check." },
    );
    setInputMonitoring(
      input
        ? { status: "ok" }
        : { status: "failed", message: "Grant Input Monitoring in System Settings, then re-check." },
    );

    if (!ax || !input) {
      setMic({ status: "pending" });
      return;
    }
    setMic({ status: "checking" });
    try {
      await api.checkMicPermission();
      setMic({ status: "ok" });
    } catch (e) {
      setMic({ status: "failed", message: String(e) });
    }
  }, []);

  useEffect(() => {
    if (step === "permissions") {
      runChecks();
    }
  }, [step, runChecks]);

  const refreshModels = useCallback(async () => {
    setModelError(null);
    try {
      const models = await api.localModelStatus();
      setLocalModels(models);
      const configured = models.find((model) => model.id === selectedModel);
      if (!configured && models.length > 0) setSelectedModel(models[0].id);
    } catch (error) {
      setModelError(readableError(error));
    }
  }, [selectedModel]);

  useEffect(() => {
    if (step === "model") refreshModels();
  }, [refreshModels, step]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<ModelProgressPayload>("local-model-progress", (event) => {
      const payload = event.payload;
      if (payload.type === "done") {
        setModelDownload(null);
        setModelError(null);
        api.localModelStatus().then(setLocalModels).catch(() => {});
        return;
      }
      if (payload.type === "error") {
        setModelDownload(null);
        setModelError(
          payload.message ?? "The model download failed. Try again.",
        );
        return;
      }
      setModelError(null);
      setModelDownload(
        `${payload.downloadedMb} MB${payload.totalMb ? `/${payload.totalMb} MB` : ""}`,
      );
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    if (step === "hotkey" && lastTranscriptId !== null) {
      finish(true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lastTranscriptId, step]);

  async function finish(success: boolean) {
    await api.setSetting("onboardingComplete", success);
    await api.setFlowbarVisible(true);
    onComplete();
  }

  async function chooseModel(id: string) {
    setSelectedModel(id);
    setModelError(null);
    try {
      await api.setLocalModel(id);
    } catch (error) {
      setModelError(readableError(error));
    }
  }

  async function downloadModel() {
    if (modelDownload !== null) return;
    setModelError(null);
    setModelDownload("starting…");
    try {
      await api.downloadLocalModel(selectedModel);
    } catch (error) {
      setModelDownload(null);
      setModelError(readableError(error));
    }
  }

  const stepIndex = ["welcome", "permissions", "model", "hotkey", "done"].indexOf(step);

  return (
    <div className="flex h-screen items-center justify-center bg-[#0d0d10]">
      <motion.div
        key={step}
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.25 }}
        className="w-full max-w-lg rounded-2xl border border-white/10 bg-white/[0.03] p-8"
      >
        <div className="mb-6 flex items-center justify-center gap-1.5">
          {["welcome", "permissions", "model", "hotkey"].map((s, i) => (
            <span
              key={s}
              className={`h-1.5 w-8 rounded-full transition-colors ${
                i <= stepIndex ? "bg-indigo-400" : "bg-white/10"
              }`}
            />
          ))}
        </div>

        {step === "welcome" && (
          <Welcome onNext={() => setStep("permissions")} />
        )}

        {step === "permissions" && (
          <Permissions
            accessibility={accessibility}
            inputMonitoring={inputMonitoring}
            mic={mic}
            onRecheck={runChecks}
            onOpenSettings={api.openAccessibilitySettings}
            onNext={() => setStep("model")}
            ready={
              accessibility.status === "ok" &&
              inputMonitoring.status === "ok" &&
              mic.status === "ok"
            }
          />
        )}

        {step === "model" && (
          <ModelSetup
            models={localModels}
            selectedModel={selectedModel}
            downloadStatus={modelDownload}
            error={modelError}
            onChoose={chooseModel}
            onDownload={downloadModel}
            onRefresh={refreshModels}
            onNext={() => setStep("hotkey")}
          />
        )}

        {step === "hotkey" && (
          <HotkeyTest hotkey={hotkey.join(" + ")} onSkip={() => finish(true)} />
        )}
      </motion.div>
    </div>
  );
}

function ModelSetup({
  models,
  selectedModel,
  downloadStatus,
  error,
  onChoose,
  onDownload,
  onRefresh,
  onNext,
}: {
  models: LocalModelInfo[];
  selectedModel: string;
  downloadStatus: string | null;
  error: string | null;
  onChoose: (id: string) => void;
  onDownload: () => void;
  onRefresh: () => void;
  onNext: () => void;
}) {
  const selected = models.find((model) => model.id === selectedModel);
  const ready = selected?.downloaded === true;

  return (
    <div>
      <h1 className="text-center text-xl font-semibold text-white">
        Set up transcription
      </h1>
      <p className="mx-auto mt-2 max-w-sm text-center text-sm text-neutral-400">
        Download one on-device model before your first dictation. The model
        runs locally after setup and does not send your audio to a cloud API.
      </p>

      <div className="mt-6 flex flex-col gap-2">
        {models.map((model) => {
          const downloading = model.id === selectedModel && downloadStatus !== null;
          return (
            <div
              key={model.id}
              className="flex items-center justify-between gap-3 rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3"
            >
              <label className="flex min-w-0 flex-1 items-center gap-3 text-left">
                <input
                  type="radio"
                  name="onboarding-model"
                  checked={selectedModel === model.id}
                  onChange={() => onChoose(model.id)}
                  className="accent-indigo-400"
                />
                <span className="min-w-0">
                  <span className="block truncate text-sm text-neutral-200">
                    {model.label}
                  </span>
                  {!model.downloaded && (
                    <span className="block text-[11px] text-neutral-600">
                      About {model.approxMb} MB
                    </span>
                  )}
                </span>
              </label>
              <div className="flex shrink-0 items-center gap-2">
                <span
                  className={`text-xs ${
                    model.downloaded ? "text-emerald-400" : "text-neutral-500"
                  }`}
                >
                  {downloading
                    ? downloadStatus
                    : model.downloaded
                      ? "Ready"
                      : "Not downloaded"}
                </span>
                {!model.downloaded && selectedModel === model.id && (
                  <button
                    type="button"
                    onClick={onDownload}
                    disabled={downloadStatus !== null}
                    className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/[0.06] disabled:cursor-wait disabled:opacity-50"
                  >
                    {downloadStatus ? "Downloading…" : "Download"}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {models.length === 0 && !error && (
        <p className="mt-4 text-center text-xs text-neutral-500">
          Loading available models…
        </p>
      )}
      {error && (
        <div className="mt-4 flex items-center justify-between gap-3 rounded-lg bg-red-500/10 px-3 py-2 text-xs text-red-300">
          <span>{error}</span>
          <button
            type="button"
            onClick={models.length > 0 ? onDownload : onRefresh}
            className="shrink-0 text-red-200 underline underline-offset-2 hover:text-white"
          >
            Retry
          </button>
        </div>
      )}

      <div className="mt-8 flex justify-center">
        <button
          type="button"
          onClick={onNext}
          disabled={!ready}
          className="rounded-lg bg-indigo-500/90 px-6 py-2.5 text-sm font-medium text-white transition enabled:hover:bg-indigo-500 disabled:opacity-40"
        >
          Continue
        </button>
      </div>
    </div>
  );
}

function Welcome({ onNext }: { onNext: () => void }) {
  return (
    <div className="text-center">
      <div className="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-gradient-to-br from-indigo-400 to-violet-600 text-2xl font-bold text-white">
        F
      </div>
      <h1 className="text-xl font-semibold text-white">Welcome to OpenFlow</h1>
      <p className="mx-auto mt-3 max-w-sm text-sm leading-relaxed text-neutral-400">
        Hold the dictation hotkey and speak naturally — ramble, pause, or
        change your mind mid-sentence. Flow understands what you mean, edits
        as you speak, and polished text appears wherever your cursor is:
        filler words removed, punctuation added, writing formatted.
      </p>
      <button
        onClick={onNext}
        className="mt-8 rounded-lg bg-indigo-500/90 px-6 py-2.5 text-sm font-medium text-white transition hover:bg-indigo-500"
      >
        Get started
      </button>
    </div>
  );
}

function Permissions({
  accessibility,
  inputMonitoring,
  mic,
  onRecheck,
  onOpenSettings,
  onNext,
  ready,
}: {
  accessibility: CheckState;
  inputMonitoring: CheckState;
  mic: CheckState;
  onRecheck: () => void;
  onOpenSettings: () => void;
  onNext: () => void;
  ready: boolean;
}) {
  return (
    <div>
      <h1 className="text-center text-xl font-semibold text-white">
        A few permissions
      </h1>
      <p className="mx-auto mt-2 max-w-sm text-center text-sm text-neutral-400">
        OpenFlow needs these to hear you and to type for you. Everything stays
        on your device.
      </p>

      <div className="mt-6 flex flex-col gap-3">
        <CheckRow
          title="Accessibility"
          subtitle="Pastes text into any app"
          state={accessibility}
          action={
            accessibility.status === "failed" ? (
              <div className="flex gap-2">
                <button
                  onClick={onOpenSettings}
                  className="rounded-md bg-indigo-500/20 px-2.5 py-1 text-xs text-indigo-300 hover:bg-indigo-500/30"
                >
                  System Settings
                </button>
                <button
                  onClick={onRecheck}
                  className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/5"
                >
                  Re-check
                </button>
              </div>
            ) : null
          }
        />
        <CheckRow
          title="Input Monitoring"
          subtitle="Lets the global hotkey work in other apps"
          state={inputMonitoring}
          action={
            inputMonitoring.status === "failed" ? (
              <div className="flex gap-2">
                <button
                  onClick={api.openInputMonitoringSettings}
                  className="rounded-md bg-indigo-500/20 px-2.5 py-1 text-xs text-indigo-300 hover:bg-indigo-500/30"
                >
                  System Settings
                </button>
                <button
                  onClick={onRecheck}
                  className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/5"
                >
                  Re-check
                </button>
              </div>
            ) : null
          }
        />
        <CheckRow
          title="Microphone"
          subtitle="Captures your voice while the hotkey is held"
          state={mic}
          action={
            mic.status === "failed" ? (
              <button
                onClick={onRecheck}
                className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 hover:bg-white/5"
              >
                Retry
              </button>
            ) : null
          }
        />
      </div>

      {mic.status === "checking" && (
        <p className="mt-4 text-center text-xs text-neutral-500">
          If a system prompt appeared, allow microphone access…
        </p>
      )}
      {accessibility.message && (
        <p className="mt-4 text-center text-xs text-red-400">
          {accessibility.message}
        </p>
      )}

      <div className="mt-8 flex justify-center">
        <button
          onClick={onNext}
          disabled={!ready}
          className="rounded-lg bg-indigo-500/90 px-6 py-2.5 text-sm font-medium text-white transition enabled:hover:bg-indigo-500 disabled:opacity-40"
        >
          Continue
        </button>
      </div>
    </div>
  );
}

function HotkeyTest({
  hotkey,
  onSkip,
}: {
  hotkey: string;
  onSkip: () => void;
}) {
  return (
    <div className="text-center">
      <h1 className="text-xl font-semibold text-white">Try it out</h1>
      <p className="mx-auto mt-3 max-w-sm text-sm leading-relaxed text-neutral-400">
        Click into any text field, hold{" "}
        <kbd className="rounded-md border border-white/15 bg-white/[0.06] px-2 py-0.5 font-mono text-xs text-neutral-200">
          {hotkey}
        </kbd>{" "}
        and say hello. Your words will be pasted when you release — and we'll
        finish setup automatically.
      </p>
      <div className="mx-auto mt-6 flex h-16 max-w-xs items-center justify-center rounded-xl border border-dashed border-white/15">
        <span className="animate-pulse text-xs text-neutral-500">
          Listening for your first dictation…
        </span>
      </div>
      <button
        onClick={onSkip}
        className="mt-8 text-xs text-neutral-500 underline-offset-2 hover:text-neutral-300 hover:underline"
      >
        Skip for now
      </button>
    </div>
  );
}

function CheckRow({
  title,
  subtitle,
  state,
  action,
}: {
  title: string;
  subtitle: string;
  state: CheckState;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3">
      <div className="min-w-0">
        <p className="text-sm text-neutral-200">{title}</p>
        <p className="text-[11px] leading-tight text-neutral-600">{subtitle}</p>
        {state.status === "failed" && state.message && !state.message.includes("System Settings") && (
          <p className="mt-0.5 truncate text-[11px] text-red-400/80">{state.message}</p>
        )}
      </div>
      <div className="ml-3 flex shrink-0 items-center gap-2">
        {action}
        <StatusDot state={state.status} />
      </div>
    </div>
  );
}

function readableError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Something went wrong. Try again.";
}

function StatusDot({ state }: { state: CheckState["status"] }) {
  if (state === "ok") {
    return (
      <span className="flex h-5 w-5 items-center justify-center rounded-full bg-emerald-500/20 text-[11px] text-emerald-400">
        ✓
      </span>
    );
  }
  if (state === "failed") {
    return (
      <span className="flex h-5 w-5 items-center justify-center rounded-full bg-red-500/20 text-[11px] text-red-400">
        ✕
      </span>
    );
  }
  return (
    <span className="h-5 w-5 animate-spin rounded-full border-2 border-white/10 border-t-indigo-400" />
  );
}
