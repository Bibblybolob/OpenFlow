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
import type { Style } from "../lib/types";

type State = "idle" | "recording" | "transcribing" | "injecting" | "paused";

interface PipelineEvent {
  type: State;
  error?: string;
}

const WAVE_BARS = 26;
const HIDE_DELAY_MS = 450;
const STATE_POLL_MS = 300;
// When no audible input reaches the capture stream for this long while
// recording, tell the user — a flat waveform otherwise looks identical to
// "the pill is broken".
const SILENCE_ALERT_MS = 2000;
const SILENCE_LEVEL_THRESHOLD = 0.05;

const pillVariants = {
  hidden: { opacity: 0, scale: 0.85, y: 12 },
  shown: { opacity: 1, scale: 1, y: 0 },
};

export default function FlowBar() {
  const [state, setState] = useState<State>("idle");
  const [hasError, setHasError] = useState(false);
  const [errorText, setErrorText] = useState<string | null>(null);
  const [wave, setWave] = useState<number[]>(() =>
    new Array(WAVE_BARS).fill(0),
  );
  const [hotkeyHint, setHotkeyHint] = useState("Right Shift");
  const [hovering, setHovering] = useState(false);
  const [micSilent, setMicSilent] = useState(false);
  const [partial, setPartial] = useState<string | null>(null);
  const [styleMenuOpen, setStyleMenuOpen] = useState(false);
  const [styles, setStyles] = useState<Style[]>([]);
  const [styleOverride, setStyleOverride] = useState<number | null>(null);
  const [overrideLabel, setOverrideLabel] = useState<string | null>(null);
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
  const lastVoiceAtRef = useRef(0);
  const hideTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const rootEl = document.getElementById("root");
    if (rootEl) rootEl.style.background = "transparent";

    let unlistenPipeline: (() => void) | undefined;
    let unlistenLevel: (() => void) | undefined;
    let unlistenStyle: (() => void) | undefined;
    let unlistenPartial: (() => void) | undefined;
    let pollId: ReturnType<typeof setInterval> | undefined;

    const applyState = (next: State) => {
      if (next === "recording" && stateRef.current !== "recording") {
        waveRef.current = new Array(WAVE_BARS).fill(0);
        lastVoiceAtRef.current = Date.now();
        setMicSilent(false);
        setPartial(null);
      }
      if (next !== "transcribing" && stateRef.current === "transcribing") {
        setPartial(null);
      }
      stateRef.current = next;
      setState(next);
    };

    listen<PipelineEvent>("pipeline", (e) => {
      applyState(e.payload.type);
      setHasError(Boolean(e.payload.error));
      setErrorText(e.payload.error ?? null);
    }).then((fn) => (unlistenPipeline = fn));

    listen<{ text: string }>("stt-partial", (e) => {
      setPartial(e.payload.text);
    }).then((fn) => (unlistenPartial = fn));

    listen<number>("audio-level", (e) => {
      if (e.payload >= SILENCE_LEVEL_THRESHOLD) {
        lastVoiceAtRef.current = Date.now();
      }
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
      clearInterval(pollId);
      clearTimeout(hideTimer.current);
      unlistenPipeline?.();
      unlistenLevel?.();
      unlistenStyle?.();
      unlistenPartial?.();
      unMoved.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (!hasError) return;
    const timer = setTimeout(() => setHasError(false), 6000);
    return () => clearTimeout(timer);
  }, [hasError]);

  const recording = state === "recording";
  const paused = state === "paused";
  const busy = state === "transcribing" || state === "injecting";
  const active = recording || paused || busy || hasError;
  const accent = accentOf(style);

  // Drive the waveform + silence detector only while recording; hidden
  // webviews pause rAF anyway, and an idle pill needs no frames.
  useEffect(() => {
    if (!recording) {
      setMicSilent(false);
      return;
    }
    let rafId = 0;
    let lastSilent: boolean | null = null;
    const smooth = () => {
      // Copy the rolling amplitude history each frame; CSS height
      // transitions turn the shifts into a scrolling waveform.
      setWave(waveRef.current.slice());
      const silent = Date.now() - lastVoiceAtRef.current > SILENCE_ALERT_MS;
      if (silent !== lastSilent) {
        lastSilent = silent;
        setMicSilent(silent);
      }
      rafId = requestAnimationFrame(smooth);
    };
    rafId = requestAnimationFrame(smooth);
    return () => cancelAnimationFrame(rafId);
  }, [recording]);

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
      // While paused the main button resumes instead of toggling a stop
      // that the FSM would ignore.
      if (stateRef.current === "paused") {
        await api.togglePause();
        return;
      }
      await api.toggleRecording();
    } catch {
      // window command unavailable — ignore
    }
  }

  async function openStyleMenu(e: React.MouseEvent) {
    e.preventDefault();
    if (stateRef.current !== "idle") return;
    try {
      const [list, current] = await Promise.all([
        api.listStyles(),
        api.getSetting<number>("styleOverride"),
      ]);
      setStyles(list.filter((st) => st.enabled));
      setStyleOverride(current ?? null);
      setOverrideLabel(
        current != null
          ? (list.find((st) => st.id === current)?.label ?? "Custom")
          : null,
      );
      setStyleMenuOpen(true);
    } catch {
      // best effort only
    }
  }

  async function pickStyle(id: number | null) {
    setStyleOverride(id);
    setOverrideLabel(
      id != null ? (styles.find((st) => st.id === id)?.label ?? null) : null,
    );
    setStyleMenuOpen(false);
    try {
      await api.setSetting("styleOverride", id);
    } catch {
      // best effort only
    }
  }

  async function onPauseClick() {
    try {
      await api.togglePause();
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
        onContextMenu={openStyleMenu}
        className={`relative flex items-center gap-3 ${radius} border px-4 py-2 shadow-2xl backdrop-blur-xl`}
        style={{
          background: pillBackground(mode, style.opacity),
          borderColor,
        }}
      >
        {styleMenuOpen && (
          <div className="absolute bottom-full left-1/2 z-10 mb-2 w-48 -translate-x-1/2 rounded-xl border border-white/10 bg-[#17171c]/95 p-1 shadow-2xl backdrop-blur-xl">
            {(
              [
                [null, "Auto (match app)"],
                [-1, "No style"],
              ] as [number | null, string][]
            ).map(([id, label]) => (
              <button
                key={label}
                onClick={() => pickStyle(id)}
                className="flex w-full items-center justify-between rounded-lg px-3 py-1.5 text-left text-xs text-neutral-300 transition hover:bg-white/10"
              >
                {label}
                {(id === null && styleOverride === null) ||
                (id === -1 && false) ? (
                  <span className="text-indigo-400">✓</span>
                ) : null}
              </button>
            ))}
            {styles.length > 0 && (
              <div className="my-1 border-t border-white/5" />
            )}
            {styles.map((st) => (
              <button
                key={st.id}
                onClick={() => pickStyle(st.id)}
                className="flex w-full items-center justify-between rounded-lg px-3 py-1.5 text-left text-xs text-neutral-300 transition hover:bg-white/10"
              >
                <span className="truncate">{st.label}</span>
                {styleOverride === st.id && (
                  <span className="text-indigo-400">✓</span>
                )}
              </button>
            ))}
          </div>
        )}

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
          title={
            paused
              ? "Resume"
              : recording
                ? "Stop (Esc to cancel)"
                : "Start dictation"
          }
          whileTap={style.animations ? { scale: 0.88 } : undefined}
          whileHover={style.animations ? { scale: 1.06 } : undefined}
          className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-full ${
            recording && !paused ? "bg-red-500/90 text-white" : "text-white"
          }`}
          style={
            recording && !paused
              ? undefined
              : {
                  background: `linear-gradient(135deg, ${accent.base}, ${shade(accent.base, -0.4)})`,
                }
          }
        >
          {recording && !paused ? (
            <span className="h-3 w-3 rounded-sm bg-white" />
          ) : paused ? (
            <svg viewBox="0 0 24 24" fill="currentColor" className="ml-0.5 h-4 w-4">
              <path d="M8 5.5v13l11-6.5-11-6.5Z" />
            </svg>
          ) : (
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
              <path d="M12 15a3.5 3.5 0 0 0 3.5-3.5V6a3.5 3.5 0 1 0-7 0v5.5A3.5 3.5 0 0 0 12 15Z" />
              <path d="M18.5 11.5a.75.75 0 0 0-1.5 0 5 5 0 0 1-10 0 .75.75 0 0 0-1.5 0 6.5 6.5 0 0 0 5.75 6.46V21h-2.25a.75.75 0 0 0 0 1.5h6a.75.75 0 0 0 0-1.5H12.75v-3.04a6.5 6.5 0 0 0 5.75-6.46Z" />
            </svg>
          )}
        </motion.button>

        {(recording || paused) && (
          <button
            onClick={onPauseClick}
            title={paused ? "Resume" : "Pause"}
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-neutral-400 transition hover:bg-white/10 hover:text-white"
          >
            {paused ? (
              <svg viewBox="0 0 24 24" fill="currentColor" className="ml-0.5 h-3.5 w-3.5">
                <path d="M8 5.5v13l11-6.5-11-6.5Z" />
              </svg>
            ) : (
              <svg viewBox="0 0 24 24" fill="currentColor" className="h-3.5 w-3.5">
                <path d="M7 5h4v14H7V5Zm6 0h4v14h-4V5Z" />
              </svg>
            )}
          </button>
        )}

        {paused ? (
          <div className="flex h-8 flex-1 items-center justify-center">
            <span className="text-xs text-neutral-400">Paused</span>
          </div>
        ) : recording ? (
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
            {micSilent && (
              <span className="ml-1 whitespace-nowrap text-[10px] text-amber-300">
                mic silent?
              </span>
            )}
          </div>
        ) : busy ? (
          partial ? (
            <p className="line-clamp-2 max-h-8 flex-1 self-center overflow-hidden text-left text-[11px] leading-[1.15rem] text-neutral-200">
              {partial}
            </p>
          ) : (
            <div className="flex h-8 flex-1 items-center justify-center gap-1.5">
              {[0, 1, 2].map((i) => (
                <span
                  key={i}
                  className="h-2 w-2 animate-bounce rounded-full"
                  style={{ animationDelay: `${i * 120}ms`, backgroundColor: accent.soft }}
                />
              ))}
            </div>
          )
        ) : (
          <span
            className={`flex max-w-44 items-center gap-2 text-xs ${
              hasError ? "text-red-300" : "text-neutral-500"
            }`}
          >
            <span className="max-w-32 truncate">
              {hasError
                ? (errorText ?? "Error — check Hub")
                : overrideLabel
                  ? `Style: ${overrideLabel}`
                  : `Hold ${hotkeyHint} or click`}
            </span>
            {hasError && (
              <button
                onClick={async () => {
                  try {
                    await api.retryLast();
                  } catch {
                    // best effort
                  }
                }}
                title="Retry transcription of the failed dictation"
                className="shrink-0 rounded-md bg-indigo-500/20 px-2 py-0.5 text-[10px] text-indigo-300 transition hover:bg-indigo-500/30"
              >
                Retry
              </button>
            )}
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
