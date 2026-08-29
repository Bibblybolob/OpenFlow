import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";
import type { Snippet as SnippetType } from "../../lib/types";

function readableError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "The snippet action failed. Try again.";
}

export default function Snippets() {
  const [snippets, setSnippets] = useState<SnippetType[]>([]);
  const [trigger, setTrigger] = useState("");
  const [body, setBody] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);

  async function refresh() {
    try {
      setSnippets(await api.listSnippets());
      setError(null);
    } catch (e) {
      setError(readableError(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh().catch(() => {});
  }, []);

  async function add() {
    if (!trigger.trim() || !body.trim()) return;
    setBusy(true);
    try {
      await api.addSnippet(trigger, body);
      setTrigger("");
      setBody("");
      await refresh();
    } catch (e) {
      setError(readableError(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Snippets</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Say the cue and FlowClone pastes the full text. Great for emails,
          links, and FAQs.
        </p>
      </div>
      {error && <p className="text-xs text-red-400">{error}</p>}

      <div className="flex flex-col gap-2">
        <input
          value={trigger}
          onChange={(e) => setTrigger(e.target.value)}
          placeholder='Spoken trigger (e.g. "my email")'
          maxLength={60}
          className="w-full rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
        />
        <textarea
          value={body}
          onChange={(e) => setBody(e.target.value)}
          placeholder="Expanded text…"
          rows={3}
          className="w-full resize-none rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
        />
        <button
          onClick={add}
          disabled={busy}
          className="self-start rounded-lg bg-indigo-500/90 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
        >
          Add snippet
        </button>
      </div>

      <div className="flex flex-col gap-2">
        {loading && <p className="text-sm text-neutral-600">Loading snippets…</p>}
        {!loading && snippets.length === 0 && (
          <p className="text-sm text-neutral-600">No snippets yet.</p>
        )}
        {snippets.map((s) => (
          <div
            key={s.id}
            className="group flex items-start justify-between gap-4 rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3"
          >
            <div className="min-w-0">
              <p className="text-sm font-medium text-neutral-200">
                “{s.trigger}”
              </p>
              <p className="mt-1 truncate text-xs text-neutral-500">{s.body}</p>
            </div>
            <button
              onClick={async () => {
                setBusy(true);
                try {
                  await api.deleteSnippet(s.id);
                  await refresh();
                } catch (error) {
                  setError(readableError(error));
                } finally {
                  setBusy(false);
                }
              }}
              disabled={busy}
              className="shrink-0 rounded-md px-2 py-1 text-xs text-neutral-500 opacity-0 transition group-hover:opacity-100 hover:bg-red-500/10 hover:text-red-400"
            >
              Delete
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
