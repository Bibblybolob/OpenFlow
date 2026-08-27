import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../lib/ipc";
import type { Stats, Transcript } from "../../lib/types";

interface HotkeyStatusEvent {
  status: string;
  detail?: string;
}

const TRANSCRIPT_PAGE_SIZE = 100;

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

export default function Home() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [transcripts, setTranscripts] = useState<Transcript[]>([]);
  const [query, setQuery] = useState("");
  const [hasMore, setHasMore] = useState(false);
  const [watcher, setWatcher] = useState("waiting-permissions");

  async function loadTranscripts(offset = 0, append = false) {
    const page = await api.listTranscripts(TRANSCRIPT_PAGE_SIZE, offset);
    setTranscripts((current) => (append ? [...current, ...page] : page));
    setHasMore(page.length === TRANSCRIPT_PAGE_SIZE);
  }

  async function refresh() {
    setStats(await api.stats());
    await loadTranscripts();
  }

  useEffect(() => {
    refresh().catch(() => {});
    api
      .hotkeyWatcherStatus()
      .then(setWatcher)
      .catch(() => {});
    let unlisten: (() => void) | undefined;
    listen<HotkeyStatusEvent>("hotkey-status", (e) => setWatcher(e.payload.status)).then(
      (fn) => (unlisten = fn),
    );
    return () => unlisten?.();
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
    const results = q.trim()
      ? await api.searchTranscripts(q)
      : await api.listTranscripts(TRANSCRIPT_PAGE_SIZE);
    setTranscripts(results);
    setHasMore(!q.trim() && results.length === TRANSCRIPT_PAGE_SIZE);
  }

  async function onDelete(id: number) {
    await api.deleteTranscript(id);
    await refresh();
  }

  async function onCopy(text: string) {
    await navigator.clipboard.writeText(text);
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
            No transcripts yet. Hold your hotkey and speak — history will appear
            here.
          </p>
        )}
        {transcripts.map((t) => (
          <div
            key={t.id}
            className="group flex items-start justify-between gap-4 rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3"
          >
            <div className="min-w-0">
              <p className="truncate text-sm text-neutral-200">{t.text}</p>
              <p className="mt-1 text-xs text-neutral-600">
                {new Date(t.createdAt).toLocaleString()} · {t.wordCount} words ·{" "}
                {t.targetApp || "unknown app"}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
              <button
                onClick={() => onCopy(t.text)}
                className="rounded-md px-2 py-1 text-xs text-neutral-400 transition hover:bg-white/5 hover:text-neutral-200"
              >
                Copy
              </button>
              <button
                onClick={async () => {
                  try {
                    await api.pasteText(t.text);
                  } catch (e) {
                    console.error(e);
                  }
                }}
                title="Paste this transcript at your cursor in the focused app"
                className="rounded-md px-2 py-1 text-xs text-neutral-400 transition hover:bg-white/5 hover:text-neutral-200"
              >
                Re-paste
              </button>
              <button
                onClick={() => onDelete(t.id)}
                className="rounded-md px-2 py-1 text-xs text-neutral-500 transition hover:bg-red-500/10 hover:text-red-400"
              >
                Delete
              </button>
            </div>
          </div>
        ))}
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
