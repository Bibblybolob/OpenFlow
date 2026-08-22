import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { api } from "../lib/ipc";

type State = "idle" | "recording" | "transcribing" | "injecting";

interface PipelineEvent {
  type: State;
  error?: string;
}

const BARS = [0.35, 0.6, 1.0, 0.75, 0.5];

export default function FlowBar() {
  const [state, setState] = useState<State>("idle");
  const [hasError, setHasError] = useState(false);
  const levelRef = useRef(0);
  const [level, setLevel] = useState(0);

  useEffect(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";

    let unlistenPipeline: (() => void) | undefined;
    let unlistenLevel: (() => void) | undefined;
    let rafId = 0;

    const smooth = () => {
      setLevel((prev) => prev + (levelRef.current - prev) * 0.35);
      rafId = requestAnimationFrame(smooth);
    };
    rafId = requestAnimationFrame(smooth);

    listen<PipelineEvent>("pipeline", (e) => {
      setState(e.payload.type);
      setHasError(Boolean(e.payload.error));
    }).then((fn) => (unlistenPipeline = fn));

    listen<number>("audio-level", (e) => {
      levelRef.current = e.payload;
    }).then((fn) => (unlistenLevel = fn));

    const win = getCurrentWebviewWindow();
    let saveTimer: ReturnType<typeof setTimeout> | undefined;
    const unMoved = win.onMoved(({ payload }) => {
      clearTimeout(saveTimer);
      saveTimer = setTimeout(() => {
        api.setSetting("flowBarPos", [payload.x, payload.y]);
      }, 400);
    });

    return () => {
      cancelAnimationFrame(rafId);
      unlistenPipeline?.();
      unlistenLevel?.();
      unMoved.then((fn) => fn());
    };
  }, []);

  async function onMicClick() {
    try {
      await api.toggleRecording();
    } catch {
      // window command unavailable — ignore
    }
  }

  const recording = state === "recording";

  useEffect(() => {
    if (!hasError) return;
    const timer = setTimeout(() => setHasError(false), 6000);
    return () => clearTimeout(timer);
  }, [hasError]);

  return (
    <div
      data-tauri-drag-region
      className="flex h-screen w-screen items-center justify-center"
    >
      <div
        data-tauri-drag-region
        className={`flex items-center gap-3 rounded-full border px-4 py-2 shadow-2xl backdrop-blur-xl transition-colors duration-200 ${
          hasError && !recording
            ? "border-red-500/40 bg-red-950/70"
            : recording
              ? "border-indigo-400/50 bg-[#17171c]/90"
              : "border-white/10 bg-[#131317]/85 hover:border-white/20"
        }`}
      >
        <button
          onClick={onMicClick}
          title={recording ? "Stop (Esc to cancel)" : "Start dictation"}
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors ${
            recording
              ? "bg-red-500/90 text-white"
              : "bg-gradient-to-br from-indigo-400 to-violet-600 text-white"
          }`}
        >
          {recording ? (
            <span className="h-3 w-3 rounded-sm bg-white" />
          ) : (
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
              <path d="M12 15a3.5 3.5 0 0 0 3.5-3.5V6a3.5 3.5 0 1 0-7 0v5.5A3.5 3.5 0 0 0 12 15Z" />
              <path d="M18.5 11.5a.75.75 0 0 0-1.5 0 5 5 0 0 1-10 0 .75.75 0 0 0-1.5 0 6.5 6.5 0 0 0 5.75 6.46V21h-2.25a.75.75 0 0 0 0 1.5h6a.75.75 0 0 0 0-1.5H12.75v-3.04a6.5 6.5 0 0 0 5.75-6.46Z" />
            </svg>
          )}
        </button>

        {recording ? (
          <div className="flex h-8 w-24 items-center justify-center gap-1">
            {BARS.map((mult, i) => (
              <span
                key={i}
                className="w-1 rounded-full bg-indigo-300 transition-[height] duration-100 ease-out"
                style={{
                  height: `${Math.max(14, Math.min(100, (level * mult + 0.08) * 100))}%`,
                }}
              />
            ))}
          </div>
        ) : state === "transcribing" || state === "injecting" ? (
          <div className="flex h-8 w-24 items-center justify-center gap-1.5">
            {[0, 1, 2].map((i) => (
              <span
                key={i}
                className="h-2 w-2 animate-bounce rounded-full bg-indigo-300"
                style={{ animationDelay: `${i * 120}ms` }}
              />
            ))}
          </div>
        ) : (
          <span className="select-none text-xs text-neutral-500">
            {hasError ? "Error — check Hub" : "Hold F5 or click"}
          </span>
        )}

        <span
          className={`h-2 w-2 shrink-0 rounded-full transition-colors ${
            recording
              ? "animate-pulse bg-red-400"
              : hasError
                ? "bg-red-400"
                : "bg-emerald-500/80"
          }`}
        />
      </div>
    </div>
  );
}
