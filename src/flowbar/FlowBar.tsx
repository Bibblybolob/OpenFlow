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

const WAVE_BARS = 4;
const HIDE_DELAY_MS = 450;
const STATE_POLL_MS = 300;
// When no audible input reaches the capture stream for this long while
// recording, tell the user — a flat waveform otherwise looks identical to
// "the pill is broken".
const SILENCE_ALERT_MS = 2000;
const SAMPLE_INTERVAL_MS = 40;
// Breathing room the Rust side adds around our reported content size. The
// pill is inset by the same amount so its glow stays inside the OS window.
const GLOW_PAD = 18;
const FIT_DEBOUNCE_MS = 80;

const pillVariants = {
  hidden: { opacity: 0, scale: 0.85, y: 12 },
  shown: { opacity: 1, scale: 1, y: 0 },
};

function useKillScrollbars() {
  useEffect(() => {
    const css = document.createElement("style");
    css.textContent =
      "html,body{overflow:hidden!important}" +
      "::-webkit-scrollbar{width:0;height:0;display:none}";
    document.head.appendChild(css);
    return () => css.remove();
  }, []);
}

export default function FlowBar() {
  useKillScrollbars();
  const [state, setState] = useState<State>("idle");
  const [hasError, setHasError] = useState(false);
  const [errorText, setErrorText] = useState<string | null>(null);
  const [hasWarning, setHasWarning] = useState(false);
  const [warningText, setWarningText] = useState<string | null>(null);
  const [warningSerial, setWarningSerial] = useState(0);
  const [wave, setWave] = useState<number[]>(() =>
    new Array(WAVE_BARS).fill(0),
  );
  const [hotkeyHint, setHotkeyHint] = useState("Right Shift");
  const [toggleMode, setToggleMode] = useState(true);
  const [hovering, setHovering] = useState(false);
  const [micSilent, setMicSilent] = useState(false);
  const [partial, setPartial] = useState<string | null>(null);
  const [styleMenuOpen, setStyleMenuOpen] = useState(false);
  const [styles, setStyles] = useState<Style[]>([]);
  const [styleOverride, setStyleOverride] = useState<
    number | null | "none"
  >(null);
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
  // Fit plumbing: the OS window shrinks/grows to wrap the pill, so the
  // pill's measured box (plus the open style menu) drives window geometry.
  const stageRef = useRef<HTMLDivElement | null>(null);
  const pillRef = useRef<HTMLDivElement | null>(null);
  const fitTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  function measureFit(): { w: number; h: number } | null {
    const root = stageRef.current;
    if (!root) return null;
    let left = 0,
      top = 0,
      right = 0,
      bottom = 0,
      any = false;
    for (const el of [
      pillRef.current,
      root.querySelector<HTMLElement>("[data-style-menu]"),
    ]) {
      if (!el) continue;
      const r = el.getBoundingClientRect();
      if (!any) {
        left = r.left;
        top = r.top;
        right = r.right;
        bottom = r.bottom;
        any = true;
      } else {
        left = Math.min(left, r.left);
        top = Math.min(top, r.top);
        right = Math.max(right, r.right);
        bottom = Math.max(bottom, r.bottom);
      }
    }
    if (!any) return null;
    return {
      // Report content only. flowbar_fit adds GLOW_PAD on both sides; adding
      // it here as well produced a large invisible click-blocking rectangle.
      w: Math.ceil(right - left),
      h: Math.ceil(bottom - top),
    };
  }

  function scheduleFit() {
    clearTimeout(fitTimer.current);
    fitTimer.current = setTimeout(() => {
      const size = measureFit();
      if (size) api.fitFlowbar(size.w, size.h).catch(() => {});
    }, FIT_DEBOUNCE_MS);
  }

  // Re-fit whenever rendered content can change the pill's footprint.
  useEffect(() => {
    if (!syncedRef.current) return;
    scheduleFit();
    const pill = pillRef.current;
    if (!pill || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(scheduleFit);
    ro.observe(pill);
    return () => {
      ro.disconnect();
      clearTimeout(fitTimer.current);
    };
  }, [
    state,
    partial,
    hotkeyHint,
    overrideLabel,
    errorText,
    warningText,
    hasWarning,
    micSilent,
    styleMenuOpen,
    style,
    shown,
  ]);

  useEffect(() => {
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    const rootEl = document.getElementById("root");
    if (rootEl) rootEl.style.background = "transparent";

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

    const unlistenPipeline = listen<PipelineEvent>("pipeline", (e) => {
      applyState(e.payload.type);
      setHasError(Boolean(e.payload.error));
      setErrorText(e.payload.error ?? null);
    });

    const unlistenWarning = listen<{ message: string }>("pipeline-warning", (e) => {
      setWarningText(e.payload.message);
      setHasWarning(true);
      setWarningSerial((serial) => serial + 1);
    });

    const unlistenPartial = listen<{ text: string }>("stt-partial", (e) => {
      setPartial(e.payload.text);
    });

    const unlistenStyle = listen("flowbar-style-changed", () => {
      loadPillStyle().then(setStyle).catch(() => {});
    });

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
        api.setSetting("flowBarPos", [payload.x, payload.y]).catch(() => {});
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
        // Fit before revealing so the window is already pill-sized.
        const size = measureFit();
        if (size) await api.fitFlowbar(size.w, size.h).catch(() => {});
        await api.setFlowbarVisible(true).catch(() => {});
        setShown(true);
      } else {
        setShown(false);
      }
      syncedRef.current = true;
      scheduleFit();
      try {
        const hk = await api.getHotkey();
        if (hk.length) setHotkeyHint(hk.join("+"));
        const hm = await api.getSetting<string>("hotkeyMode");
        setToggleMode(hm !== "push_to_talk");
      } catch {
        // keep default hint
      }
    })();

    return () => {
      clearInterval(pollId);
      clearTimeout(hideTimer.current);
      unlistenPipeline.then((fn) => fn()).catch(() => {});
      unlistenWarning.then((fn) => fn()).catch(() => {});
      unlistenStyle.then((fn) => fn()).catch(() => {});
      unlistenPartial.then((fn) => fn()).catch(() => {});
      unMoved.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    if (!hasError) return;
    const timer = setTimeout(() => setHasError(false), 6000);
    return () => clearTimeout(timer);
  }, [hasError]);

  useEffect(() => {
    if (!hasWarning) return;
    const timer = setTimeout(() => setHasWarning(false), 8000);
    return () => clearTimeout(timer);
  }, [hasWarning, warningSerial]);

  const recording = state === "recording";
  const paused = state === "paused";
  const busy = state === "transcribing" || state === "injecting";
  const active = recording || paused || busy || hasError || hasWarning;
  const accent = accentOf(style);

  // Waveform + silence detector, driven by polling the Rust-side level
  // instead of subscribing to events: WebKit starves event delivery and
  // requestAnimationFrame in this non-activating overlay window, while
  // invoke + setInterval keep working. ~25fps with a 70ms CSS transition.
  useEffect(() => {
    if (!recording) {
      setMicSilent(false);
      return;
    }
    let lastSilent: boolean | null = null;
    const sample = async () => {
      // Rust computes both a fixed-reference display envelope and the
      // floor-relative voice decision; the pill just renders them.
      const { bar, voiced } = await api
        .micLevel()
        .catch(() => ({ bar: 0, voiced: false }));
      if (voiced) {
        lastVoiceAtRef.current = Date.now();
      }
      const next = waveRef.current.slice(1);
      next.push(bar);
      waveRef.current = next;
      setWave(next);
      const silent = Date.now() - lastVoiceAtRef.current > SILENCE_ALERT_MS;
      if (silent !== lastSilent) {
        lastSilent = silent;
        setMicSilent(silent);
      }
    };
    sample();
    const samplerId = setInterval(sample, SAMPLE_INTERVAL_MS);
    return () => clearInterval(samplerId);
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
        api.getSetting<number | "none">("styleOverride"),
      ]);
      setStyles(list.filter((st) => st.enabled));
      const normalized = current === -1 ? "none" : (current ?? null);
      setStyleOverride(normalized);
      setOverrideLabel(
        normalized === "none"
          ? "No style"
          : typeof normalized === "number"
            ? (list.find((st) => st.id === normalized)?.label ?? "Custom")
            : null,
      );
      setStyleMenuOpen(true);
    } catch {
      // best effort only
    }
  }

  async function pickStyle(id: number | null | "none") {
    setStyleOverride(id);
    setOverrideLabel(
      id === "none"
        ? "No style"
        : typeof id === "number"
          ? (styles.find((st) => st.id === id)?.label ?? null)
          : null,
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
  const mode = hasError && !recording
    ? "error"
    : hasWarning && !recording
      ? "warning"
      : recording || busy
        ? "active"
        : "idle";
  const borderColor =
    mode === "error"
      ? rgba("#f87171", 0.5)
      : mode === "warning"
        ? rgba("#fbbf24", 0.5)
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
      ref={stageRef}
      className="relative h-screen w-screen overflow-hidden"
    >
      <motion.div
        ref={pillRef}
        data-pill
        data-tauri-drag-region
        variants={pillVariants}
        initial="hidden"
        animate={shown ? "shown" : "hidden"}
        transition={spring}
        onAnimationComplete={(def) => {
          onHiddenAnimationDone(def);
          // Entrance settles at scale 1 — re-fit once the transform is done
          // so the window wraps the final geometry.
          if (def === "shown") scheduleFit();
        }}
        onMouseEnter={() => setHovering(true)}
        onMouseLeave={() => setHovering(false)}
        onContextMenu={openStyleMenu}
        className={`absolute flex items-center gap-2 ${radius} border px-3 py-1.5 shadow-2xl`}
        style={{
          left: GLOW_PAD,
          top: GLOW_PAD,
          background: pillBackground(mode, style.opacity),
          borderColor,
        }}
      >
        {styleMenuOpen && (
          <div
            data-style-menu
            className="absolute top-full left-0 z-10 mt-2 w-48 rounded-xl border border-white/10 bg-[#17171c]/95 p-1 shadow-2xl"
          >
            {(
              [
                [null, "Auto (match app)"],
                ["none", "No style"],
              ] as [number | null | "none", string][]
            ).map(([id, label]) => (
              <button
                key={label}
                onClick={() => pickStyle(id)}
                className="flex w-full items-center justify-between rounded-lg px-3 py-1.5 text-left text-xs text-neutral-300 transition hover:bg-white/10"
              >
                {label}
                {(id === null && styleOverride === null) ||
                (id === "none" && styleOverride === "none") ? (
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
          className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full ${
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
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-neutral-400 transition hover:bg-white/10 hover:text-white"
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
          <div className="flex h-6 flex-1 items-center justify-center">
            <span className="text-xs text-neutral-400">Paused</span>
          </div>
        ) : recording ? (
          <div className="relative flex h-6 flex-1 items-center justify-center gap-[3px]">
            {wave.map((v, i) => {
              const boosted = Math.min(1, v * 1.35);
              return (
                <span
                  key={i}
                  className="w-[5px] shrink-0 rounded-full"
                  style={{
                    height: `${Math.max(14, boosted * 100)}%`,
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
            <p className="line-clamp-2 max-h-7 flex-1 self-center overflow-hidden text-left text-[11px] leading-[1.05rem] text-neutral-200">
              {partial}
            </p>
          ) : (
            <div className="flex h-6 flex-1 items-center justify-center gap-1.5">
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
              hasError
                ? "text-red-300"
                : hasWarning
                  ? "text-amber-300"
                  : "text-neutral-500"
            }`}
          >
            <span className="max-w-32 truncate">
              {hasError
                ? (errorText ?? "Error — check Hub")
                : hasWarning
                  ? (warningText ?? "Warning — check Hub")
                : overrideLabel
                  ? `Style: ${overrideLabel}`
                  : `${toggleMode ? "Press" : "Hold"} ${hotkeyHint} or click`}
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
                : hasWarning
                  ? { backgroundColor: "#fbbf24" }
                : { backgroundColor: rgba(accent.base, 0.85) }
          }
        />
      </motion.div>
    </div>
  );
}
