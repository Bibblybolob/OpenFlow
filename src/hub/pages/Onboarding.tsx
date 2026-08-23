import { useCallback, useEffect, useState } from "react";
import { motion } from "framer-motion";
import { api } from "../../lib/ipc";
import { usePipelineState } from "../usePipelineState";

type Step = "welcome" | "permissions" | "hotkey" | "done";

interface CheckState {
  status: "pending" | "checking" | "ok" | "failed";
  message?: string;
}

export default function Onboarding({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<Step>("welcome");
  const [accessibility, setAccessibility] = useState<CheckState>({ status: "pending" });
  const [mic, setMic] = useState<CheckState>({ status: "pending" });
  const [hotkey, setHotkey] = useState<string[]>([]);
  const { lastTranscriptId } = usePipelineState();

  useEffect(() => {
    api.getHotkey().then(setHotkey).catch(() => setHotkey(["Right Shift"]));
    api.setFlowbarVisible(false);
  }, []);

  const runChecks = useCallback(async () => {
    setAccessibility({ status: "checking" });
    const ax = await api.accessibilityStatus();
    setAccessibility(
      ax
        ? { status: "ok" }
        : { status: "failed", message: "Grant Accessibility in System Settings, then re-check." },
    );

    if (!ax) {
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

  const stepIndex = ["welcome", "permissions", "hotkey", "done"].indexOf(step);

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
          {["welcome", "permissions", "hotkey"].map((s, i) => (
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
            mic={mic}
            onRecheck={runChecks}
            onOpenSettings={api.openAccessibilitySettings}
            onNext={() => setStep("hotkey")}
            ready={accessibility.status === "ok" && mic.status === "ok"}
          />
        )}

        {step === "hotkey" && (
          <HotkeyTest hotkey={hotkey.join(" + ")} onSkip={() => finish(true)} />
        )}
      </motion.div>
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
        Hold the dictation hotkey, speak naturally, and polished text appears
        wherever your cursor is — filler words removed, punctuation fixed.
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
  mic,
  onRecheck,
  onOpenSettings,
  onNext,
  ready,
}: {
  accessibility: CheckState;
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
          subtitle="Global hotkey + pasting into any app"
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
