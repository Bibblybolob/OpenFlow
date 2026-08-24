import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../../lib/ipc";
import type { Stats, Transcript } from "../../lib/types";

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
  const [watcher, setWatcher] = useState("waiting-permissions");

  async function refresh() {
    setStats(await api.stats());
    setTranscripts(await api.listTranscripts(100));
  }

  useEffect(() => {
    refresh();
    api
      .hotkeyWatcherStatus()
      .then(setWatcher)
      .catch(() => {});
    let unlisten: (() => void) | undefined;
    listen<string>("hotkey-status", (e) => setWatcher(e.payload)).then(
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

  async function onSearch(q: string) {
    setQuery(q);
    setTranscripts(
      q.trim() ? await api.searchTranscripts(q) : await api.listTranscripts(100),
    );
  }

  async function addSample() {
    await api.insertTranscript({
      text: "This is a sample dictation to verify the pipeline.",
      rawText: "um this is a uh sample dictation to like verify the pipeline",
      language: "en",
      durationMs: 3200,
      targetApp: "FlowClone",
    });
    refresh();
  }

  async function onDelete(id: number) {
    await api.deleteTranscript(id);
    refresh();
  }

  async function onCopy(text: string) {
    await navigator.clipboard.writeText(text);
  }

  const visible = transcripts.filter((t) =>
    query.trim()
      ? true
      : new Date(t.createdAt).toDateString() === new Date().toDateString(),
  );

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      {!watcherReady && (
        <button
          onClick={() => {
            (watcher === "waiting-accessibility"
              ? api.openAccessibilitySettings()
              : api.openInputMonitoringSettings()
            ).catch(() => {});
          }}
          className="flex items-center justify-between rounded-xl border border-amber-500/30 bg-amber-500/10 px-5 py-3 text-left transition hover:bg-amber-500/[0.16]"
        >
          <span>
            <span className="block text-sm font-medium text-amber-200">
              {watcherBanner.title}
            </span>
            <span className="mt-0.5 block text-xs text-amber-200/70">
              {watcherBanner.detail}
            </span>
          </span>
          <span className="text-xs font-medium text-amber-300">Fix →</span>
        </button>
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
        <button
          onClick={addSample}
          className="rounded-lg bg-indigo-500/90 px-3 py-1.5 text-sm font-medium text-white transition hover:bg-indigo-500"
        >
          Add sample dictation
        </button>
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
        {visible.length === 0 && (
          <p className="text-sm text-neutral-600">
            No transcripts yet. Hold your hotkey and speak — history will appear
            here.
          </p>
        )}
        {visible.map((t) => (
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
      </div>
    </div>
  );
}
