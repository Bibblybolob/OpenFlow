import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { motion } from "framer-motion";
import { api } from "../lib/ipc";
import {
  accentOf,
  loadPillStyle,
  pillBackground,
  pillRadius,
  rgba,
  shade,
  type PillStyle,
} from "../lib/pillStyle";

type State = "idle" | "recording" | "transcribing" | "injecting";

interface PipelineEvent {
  type: State;
  error?: string;
}

const WAVE_BARS = 26;
const HIDE_DELAY_MS = 450;
const STATE_POLL_MS = 300;

const pillVariants = {
  hidden: { opacity: 0, scale: 0.85, y: 12 },
  shown: { opacity: 1, scale: 1, y: 0 },
};

export default function FlowBar() {
  const [state, setState] = useState<State>("idle");
  const [hasError, setHasError] = useState(false);
  const [wave, setWave] = useState<number[]>(() =>
    new Array(WAVE_BARS).fill(0),
  );
  const [hotkeyHint, setHotkeyHint] = useState("F5");
  const [hovering, setHovering] = useState(false);
  const [style, setStyle] = useState<PillStyle>({
    shape: "pill",
    accent: "indigo",
    opacity: 88,
    animations: true,
    autoHide: true,
  });
  const [shown, setShown] = useState(false);

  const stateRef = useRef<State>("idle");
  const waveRef = useRef<number[]>(new Array(WAVE_BARS).fill(0));
  const syncedRef = useRef(false);
  const wantVisibleRef = useRef(false);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const rootEl = document.getElementById("root");
    if (rootEl) rootEl.style.background = "transparent";

    let unlistenPipeline: (() => void) | undefined;
    let unlistenLevel: (() => void) | undefined;
    let unlistenStyle: (() => void) | undefined;
    let rafId = 0;
    let pollId: ReturnType<typeof setInterval> | undefined;

    const applyState = (next: State) => {
      if (next === "recording" && stateRef.current !== "recording") {
        waveRef.current = new Array(WAVE_BARS).fill(0);
      }
      stateRef.current = next;
      setState(next);
    };

    const smooth = () => {
      // Copy the rolling amplitude history each frame; CSS height
      // transitions turn the shifts into a scrolling waveform.
      setWave(waveRef.current.slice());
      rafId = requestAnimationFrame(smooth);
    };
    rafId = requestAnimationFrame(smooth);

    listen<PipelineEvent>("pipeline", (e) => {
      applyState(e.payload.type);
      setHasError(Boolean(e.payload.error));
    }).then((fn) => (unlistenPipeline = fn));

    listen<number>("audio-level", (e) => {
      const next = waveRef.current.slice(1);
      next.push(e.payload);
      waveRef.current = next;
    }).then((fn) => (unlistenLevel = fn));

    listen("flowbar-style-changed", () => {
      loadPillStyle().then(setStyle);
    }).then((fn) => (unlistenStyle = fn));

    // Safety net: events can be missed while this hidden window is still
    // loading (or if its webview throttles listeners), so periodically
    // reconcile against the Rust-side FSM directly.
    pollId = setInterval(async () => {
      const current = (await api.pipelineStatus().catch(() => null)) as
        | State
        | null;
      if (current && current !== stateRef.current) applyState(current);
    }, STATE_POLL_MS);

    const win = getCurrentWebviewWindow();
    let saveTimer: ReturnType<typeof setTimeout> | undefined;
    const unMoved = win.onMoved(({ payload }) => {
      clearTimeout(saveTimer);
      saveTimer = setTimeout(() => {
        api.setSetting("flowBarPos", [payload.x, payload.y]);
      }, 400);
    });

    (async () => {
      const loaded = await loadPillStyle();
      setStyle(loaded);
      const current = (await api.pipelineStatus().catch(() => "idle")) as State;
      applyState(current);
      const visibleAtLaunch = !(loaded.autoHide && current === "idle");
      wantVisibleRef.current = visibleAtLaunch;
      if (visibleAtLaunch) {
        await api.setFlowbarVisible(true).catch(() => {});
        setShown(true);
      } else {
        setShown(false);
      }
      syncedRef.current = true;
      try {
        const hk = await api.getHotkey();
        if (hk.length) setHotkeyHint(hk.join("+"));
      } catch {
        // keep default hint
      }
    })();

    return () => {
      cancelAnimationFrame(rafId);
      clearInterval(pollId);
      clearTimeout(hideTimer.current);
      unlistenPipeline?.();
      unlistenLevel?.();
      unlistenStyle?.();
      unMoved.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (!hasError) return;
    const timer = setTimeout(() => setHasError(false), 6000);
    return () => clearTimeout(timer);
  }, [hasError]);

  const recording = state === "recording";
  const busy = state === "transcribing" || state === "injecting";
  const active = recording || busy || hasError;
  const accent = accentOf(style);

  useEffect(() => {
    if (!syncedRef.current) return;
    const wantVisible = !style.autoHide || active;
    wantVisibleRef.current = wantVisible;
    clearTimeout(hideTimer.current);
    if (wantVisible) {
      api.setFlowbarVisible(true).catch(() => {});
      setShown(true);
      return;
    }
    hideTimer.current = setTimeout(() => {
      if (style.animations) {
        setShown(false);
      } else {
        api.setFlowbarVisible(false).catch(() => {});
        setShown(false);
      }
    }, HIDE_DELAY_MS);
  }, [active, style]);

  function onHiddenAnimationDone(def?: unknown) {
    if (def !== "hidden") return;
    if (!wantVisibleRef.current) api.setFlowbarVisible(false).catch(() => {});
  }

  async function onMicClick() {
    try {
      await api.toggleRecording();
    } catch {
      // window command unavailable — ignore
    }
  }

  const radius = pillRadius(style.shape);
  const mode = hasError && !recording ? "error" : recording || busy ? "active" : "idle";
  const borderColor =
    mode === "error"
      ? rgba("#f87171", 0.5)
      : mode === "active"
        ? rgba(accent.border, 0.55)
        : hovering
          ? "rgba(255,255,255,0.22)"
          : "rgba(255,255,255,0.12)";

  const spring = style.animations
    ? { type: "spring" as const, stiffness: 420, damping: 28 }
    : { duration: 0 };

  return (
    <div
      data-tauri-drag-region
      className="flex h-screen w-screen items-center justify-center"
    >
      <motion.div
        data-tauri-drag-region
        variants={pillVariants}
        initial="hidden"
        animate={shown ? "shown" : "hidden"}
        transition={spring}
        onAnimationComplete={onHiddenAnimationDone}
        onMouseEnter={() => setHovering(true)}
        onMouseLeave={() => setHovering(false)}
        className={`relative flex items-center gap-3 ${radius} border px-4 py-2 shadow-2xl backdrop-blur-xl`}
        style={{
          background: pillBackground(mode, style.opacity),
          borderColor,
        }}
      >
        {recording && style.animations && (
          <motion.span
            aria-hidden
            className={`pointer-events-none absolute inset-0 ${radius}`}
            animate={{
              boxShadow: [
                `0 0 0 0px rgba(${accent.glow},0.45), 0 0 14px 2px rgba(${accent.glow},0.20)`,
                `0 0 0 7px rgba(${accent.glow},0.00), 0 0 26px 6px rgba(${accent.glow},0.38)`,
                `0 0 0 0px rgba(${accent.glow},0.45), 0 0 14px 2px rgba(${accent.glow},0.20)`,
              ],
            }}
            transition={{ duration: 1.8, repeat: Infinity, ease: "easeInOut" }}
          />
        )}

        <motion.button
          onClick={onMicClick}
          title={recording ? "Stop (Esc to cancel)" : "Start dictation"}
          whileTap={style.animations ? { scale: 0.88 } : undefined}
          whileHover={style.animations ? { scale: 1.06 } : undefined}
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full ${
            recording ? "bg-red-500/90 text-white" : "text-white"
          }`}
          style={
            recording
              ? undefined
              : {
                  background: `linear-gradient(135deg, ${accent.base}, ${shade(accent.base, -0.4)})`,
                }
          }
        >
          {recording ? (
            <span className="h-3 w-3 rounded-sm bg-white" />
          ) : (
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
              <path d="M12 15a3.5 3.5 0 0 0 3.5-3.5V6a3.5 3.5 0 1 0-7 0v5.5A3.5 3.5 0 0 0 12 15Z" />
              <path d="M18.5 11.5a.75.75 0 0 0-1.5 0 5 5 0 0 1-10 0 .75.75 0 0 0-1.5 0 6.5 6.5 0 0 0 5.75 6.46V21h-2.25a.75.75 0 0 0 0 1.5h6a.75.75 0 0 0 0-1.5H12.75v-3.04a6.5 6.5 0 0 0 5.75-6.46Z" />
            </svg>
          )}
        </motion.button>

        {recording ? (
          <div className="flex h-8 flex-1 items-center justify-center gap-[3px]">
            {wave.map((v, i) => {
              const boosted = Math.min(1, v * 1.35);
              return (
                <span
                  key={i}
                  className="w-[3px] shrink-0 rounded-full"
                  style={{
                    height: `${Math.max(9, boosted * 100)}%`,
                    backgroundColor: accent.soft,
                    opacity: 0.45 + boosted * 0.55,
                    transition: "height 70ms linear",
                  }}
                />
              );
            })}
          </div>
        ) : busy ? (
          <div className="flex h-8 w-24 items-center justify-center gap-1.5">
            {[0, 1, 2].map((i) => (
              <span
                key={i}
                className="h-2 w-2 animate-bounce rounded-full"
                style={{ animationDelay: `${i * 120}ms`, backgroundColor: accent.soft }}
              />
            ))}
          </div>
        ) : (
          <span
            className={`max-w-40 truncate select-none text-xs ${
              hasError ? "text-red-300" : "text-neutral-500"
            }`}
          >
            {hasError ? "Error — check Hub" : `Hold ${hotkeyHint} or click`}
          </span>
        )}

        <span
          className={`h-2 w-2 shrink-0 rounded-full transition-colors ${
            recording && style.animations ? "animate-pulse bg-red-400" : ""
          }`}
          style={
            recording
              ? { backgroundColor: "#f87171" }
              : hasError
                ? { backgroundColor: "#f87171" }
                : { backgroundColor: rgba(accent.base, 0.85) }
          }
        />
      </motion.div>
    </div>
  );
}
