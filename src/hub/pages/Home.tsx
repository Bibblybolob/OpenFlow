import { Fragment, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../lib/ipc";
import type { Stats, Transcript } from "../../lib/types";

interface HotkeyStatusEvent {
  status: string;
  detail?: string;
}

const TRANSCRIPT_PAGE_SIZE = 100;

function readableError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Re-paste failed. Copy the transcript and paste it manually.";
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex-1 rounded-xl border border-white/5 bg-white/[0.03] px-5 py-4">
      <div className="text-xs uppercase tracking-wider text-neutral-500">
        {label}
      </div>
      <div className="mt-1 text-2xl font-semibold text-white">{value}</div>
    </div>
  );
}

function dayKey(createdAt: string): string {
  return new Date(createdAt).toDateString();
}

function dayLabel(createdAt: string): string {
  const date = new Date(createdAt);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (dayKey(createdAt) === today.toDateString()) return "Today";
  if (dayKey(createdAt) === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
    year: date.getFullYear() === today.getFullYear() ? undefined : "numeric",
  });
}

export default function Home() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [transcripts, setTranscripts] = useState<Transcript[]>([]);
  const [query, setQuery] = useState("");
  const [hasMore, setHasMore] = useState(false);
  const [watcher, setWatcher] = useState("waiting-permissions");
  const [historyActionError, setHistoryActionError] = useState<{
    id: number;
    message: string;
  } | null>(null);
  const historyRequest = useRef(0);

  async function loadTranscripts(offset = 0, append = false) {
    const request = ++historyRequest.current;
    const page = await api.listTranscripts(TRANSCRIPT_PAGE_SIZE, offset);
    if (request !== historyRequest.current) return;
    setTranscripts((current) => (append ? [...current, ...page] : page));
    setHasMore(page.length === TRANSCRIPT_PAGE_SIZE);
  }

  async function refresh() {
    // Start history immediately so a search typed during the stats request
    // can supersede it instead of being overwritten by a late initial load.
    const history = loadTranscripts();
    setStats(await api.stats());
    await history;
  }

  useEffect(() => {
    refresh().catch(() => {});
    api
      .hotkeyWatcherStatus()
      .then(setWatcher)
      .catch(() => {});
    const unlisten = listen<HotkeyStatusEvent>("hotkey-status", (e) =>
      setWatcher(e.payload.status),
    );
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const watcherReady = watcher === "ready";

  const watcherBanner = (() => {
    switch (true) {
      case watcher.startsWith("unavailable"):
        return {
          title: "Hotkey inactive — backend unavailable",
          detail: watcher.replace("unavailable:", ""),
        };
      case watcher === "waiting-accessibility":
        return {
          title: "Hotkey inactive — grant Accessibility",
          detail:
            "Click to open System Settings → Privacy → Accessibility, add FlowClone and enable it",
        };
      case watcher === "waiting-input-monitoring":
        return {
          title: "Hotkey inactive — grant Input Monitoring",
          detail:
            "Click to open System Settings → Privacy → Input Monitoring, add FlowClone and enable it",
        };
      default:
        return {
          title: "Hotkey inactive — starting…",
          detail: "",
        };
    }
  })();
  const permissionFix =
    watcher === "waiting-accessibility"
      ? api.openAccessibilitySettings
      : watcher === "waiting-input-monitoring"
        ? api.openInputMonitoringSettings
        : null;

  async function onSearch(q: string) {
    setQuery(q);
    const request = ++historyRequest.current;
    const trimmed = q.trim();
    const results = trimmed
      ? await api.searchTranscripts(trimmed)
      : await api.listTranscripts(TRANSCRIPT_PAGE_SIZE);
    // SQLite requests can complete out of order while a user types quickly.
    // Only the newest query is allowed to replace the visible history.
    if (request !== historyRequest.current) return;
    setTranscripts(results);
    setHasMore(!trimmed && results.length === TRANSCRIPT_PAGE_SIZE);
  }

  async function onDelete(id: number) {
    await api.deleteTranscript(id);
    setTranscripts((current) => current.filter((transcript) => transcript.id !== id));
    setStats(await api.stats());
  }

  async function onCopy(text: string) {
    await navigator.clipboard.writeText(text);
  }

  async function onFlag(id: number, flagged: boolean) {
    await api.setFlagged(id, !flagged);
    setTranscripts((current) =>
      current.map((transcript) =>
        transcript.id === id
          ? { ...transcript, flagged: !flagged }
          : transcript,
      ),
    );
  }

  async function onRepaste(transcript: Transcript) {
    setHistoryActionError(null);
    try {
      await api.pasteText(transcript.text, transcript.targetApp);
    } catch (error) {
      setHistoryActionError({ id: transcript.id, message: readableError(error) });
    }
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      {!watcherReady && (
        <div
          className={`flex items-center justify-between rounded-xl border border-amber-500/30 bg-amber-500/10 px-5 py-3 text-left ${
            permissionFix ? "transition hover:bg-amber-500/[0.16]" : ""
          }`}
        >
          <span>
            <span className="block text-sm font-medium text-amber-200">
              {watcherBanner.title}
            </span>
            <span className="mt-0.5 block text-xs text-amber-200/70">
              {watcherBanner.detail}
            </span>
          </span>
          {permissionFix && (
            <button
              type="button"
              onClick={() => permissionFix().catch(() => {})}
              className="text-xs font-medium text-amber-300 hover:text-amber-100"
            >
              Fix →
            </button>
          )}
        </div>
      )}
      <div className="rounded-xl border border-white/5 bg-gradient-to-br from-indigo-500/[0.07] to-violet-500/[0.04] px-5 py-4">
        <p className="text-sm font-medium text-neutral-100">Speak naturally</p>
        <p className="mt-1 text-xs leading-relaxed text-neutral-400">
          Ramble, pause, or change your mind mid-sentence. Flow understands
          what you mean, not just what you say — and it edits as you speak,
          so the text reads like you wrote it, not like you spoke it: filler
          words removed, punctuation added, writing formatted.
        </p>
      </div>

      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Home</h1>
      </div>

      <div className="flex gap-4">
        <StatCard
          label="Words dictated"
          value={stats ? stats.totalWords.toLocaleString() : "—"}
        />
        <StatCard
          label="Dictations"
          value={stats ? stats.transcriptCount.toLocaleString() : "—"}
        />
        <StatCard
          label="Streak"
          value={stats ? `${stats.streakDays} day${stats.streakDays === 1 ? "" : "s"}` : "—"}
        />
      </div>

      <input
        value={query}
        onChange={(e) => onSearch(e.target.value)}
        placeholder="Search transcripts…"
        className="w-full rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
      />

      <div className="flex flex-col gap-3">
        {transcripts.length === 0 && (
          <p className="text-sm text-neutral-600">
            No transcripts yet. Use your hotkey and speak — history will appear
            here.
          </p>
        )}
        {transcripts.map((t, index) => {
          const showDay = index === 0 || dayKey(t.createdAt) !== dayKey(transcripts[index - 1].createdAt);
          return (
            <Fragment key={t.id}>
              {showDay && (
                <h2 className="pt-2 text-xs font-medium tracking-wider text-neutral-500 uppercase">
                  {dayLabel(t.createdAt)}
                </h2>
              )}
              <div className="group flex items-start justify-between gap-4 rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3">
                <div className="min-w-0">
                  <p className="truncate text-sm text-neutral-200">{t.text}</p>
                  <p className="mt-1 text-xs text-neutral-600">
                    {new Date(t.createdAt).toLocaleString()} · {t.wordCount} words ·{" "}
                    {t.targetApp || "unknown app"}
                  </p>
                  {historyActionError?.id === t.id && (
                    <p className="mt-1 text-xs text-red-400">
                      {historyActionError.message}
                    </p>
                  )}
                </div>
                <div
                  className={`flex shrink-0 items-center gap-1 transition group-hover:opacity-100 group-focus-within:opacity-100 ${
                    t.flagged ? "opacity-100" : "opacity-0"
                  }`}
                >
                  <button
                    type="button"
                    onClick={() => onFlag(t.id, t.flagged)}
                    aria-label={t.flagged ? "Unpin transcript" : "Pin transcript"}
                    title={t.flagged ? "Unpin transcript" : "Pin transcript"}
                    className={`rounded-md px-2 py-1 text-sm transition hover:bg-white/5 ${
                      t.flagged ? "text-amber-400" : "text-neutral-600 hover:text-neutral-300"
                    }`}
                  >
                    ★
                  </button>
                  <button
                    type="button"
                    onClick={() => onCopy(t.text)}
                    className="rounded-md px-2 py-1 text-xs text-neutral-400 transition hover:bg-white/5 hover:text-neutral-200"
                  >
                    Copy
                  </button>
                  <button
                    type="button"
                    onClick={() => onRepaste(t)}
                    disabled={!t.targetApp}
                    title={
                      t.targetApp
                        ? `Switch to ${t.targetApp} and paste this transcript`
                        : "The original target app is unknown"
                    }
                    className="rounded-md px-2 py-1 text-xs text-neutral-400 transition hover:bg-white/5 hover:text-neutral-200 disabled:cursor-not-allowed disabled:opacity-40"
                  >
                    Re-paste
                  </button>
                  <button
                    type="button"
                    onClick={() => onDelete(t.id)}
                    className="rounded-md px-2 py-1 text-xs text-neutral-500 transition hover:bg-red-500/10 hover:text-red-400"
                  >
                    Delete
                  </button>
                </div>
              </div>
            </Fragment>
          );
        })}
        {hasMore && !query.trim() && (
          <button
            type="button"
            onClick={() => loadTranscripts(transcripts.length, true)}
            className="self-center rounded-lg border border-white/10 px-4 py-2 text-xs text-neutral-400 transition hover:bg-white/[0.05] hover:text-neutral-200"
          >
            Load older transcripts
          </button>
        )}
      </div>
    </div>
  );
}
