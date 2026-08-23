import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";
import type { DictionaryEntry, VocabSuggestion } from "../../lib/types";

export default function Dictionary() {
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [suggestions, setSuggestions] = useState<VocabSuggestion[]>([]);
  const [term, setTerm] = useState("");
  const [replacement, setReplacement] = useState("");

  async function refresh() {
    setEntries(await api.listDictionary());
    setSuggestions(await api.listVocabSuggestions().catch(() => []));
  }

  useEffect(() => {
    refresh();
  }, []);

  async function add() {
    if (!term.trim()) return;
    await api.addDictionaryTerm(term, replacement.trim() || undefined);
    setTerm("");
    setReplacement("");
    refresh();
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Dictionary</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Custom words, names, and jargon FlowClone should get right. Add a
          replacement to auto-correct a common misspelling.
        </p>
      </div>

      {suggestions.length > 0 && (
        <section className="flex flex-col gap-2 rounded-xl border border-amber-400/20 bg-amber-400/[0.04] p-4">
          <h2 className="text-xs font-medium uppercase tracking-wider text-amber-300/80">
            Learned vocabulary — needs review
          </h2>
          <p className="text-xs text-neutral-500">
            Flow heard these corrections more than once. Accept to always
            spell them this way.
          </p>
          {suggestions.map((s) => (
            <div
              key={s.id}
              className="flex items-center justify-between gap-4 rounded-lg bg-white/[0.03] px-3 py-2"
            >
              <div className="min-w-0 text-sm">
                <span className="text-neutral-200">{s.term}</span>
                <span className="ml-2 text-xs text-neutral-600">
                  heard as “{s.rawForm}” · {s.occurrences}×
                </span>
              </div>
              <div className="flex shrink-0 gap-1">
                <button
                  onClick={async () => {
                    await api.acceptVocabSuggestion(s.id);
                    refresh();
                  }}
                  className="rounded-md bg-emerald-500/20 px-2.5 py-1 text-xs text-emerald-300 transition hover:bg-emerald-500/30"
                >
                  Accept
                </button>
                <button
                  onClick={async () => {
                    await api.dismissVocabSuggestion(s.id);
                    refresh();
                  }}
                  className="rounded-md px-2.5 py-1 text-xs text-neutral-500 transition hover:bg-white/5"
                >
                  Dismiss
                </button>
              </div>
            </div>
          ))}
        </section>
      )}

      <div className="flex gap-2">
        <input
          value={term}
          onChange={(e) => setTerm(e.target.value)}
          placeholder="Word or term (e.g. Kubernetes)"
          maxLength={30}
          className="flex-1 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
        />
        <input
          value={replacement}
          onChange={(e) => setReplacement(e.target.value)}
          placeholder="Correct spelling (optional)"
          maxLength={30}
          className="flex-1 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
        />
        <button
          onClick={add}
          className="rounded-lg bg-indigo-500/90 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
        >
          Add
        </button>
      </div>

      <div className="flex flex-col gap-2">
        {entries.length === 0 && (
          <p className="text-sm text-neutral-600">
            No entries yet. Words you star get higher priority.
          </p>
        )}
        {entries.map((e) => (
          <div
            key={e.id}
            className="group flex items-center justify-between rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3"
          >
            <div className="min-w-0">
              <p className="text-sm text-neutral-200">{e.term}</p>
              {e.replacement && (
                <p className="text-xs text-neutral-500">→ {e.replacement}</p>
              )}
            </div>
            <div className="flex items-center gap-1">
              <button
                onClick={async () => {
                  await api.setDictionaryStarred(e.id, !e.starred);
                  refresh();
                }}
                className={`rounded-md px-2 py-1 text-xs transition hover:bg-white/5 ${
                  e.starred ? "text-amber-400" : "text-neutral-600"
                }`}
              >
                ★
              </button>
              <button
                onClick={async () => {
                  await api.deleteDictionaryTerm(e.id);
                  refresh();
                }}
                className="rounded-md px-2 py-1 text-xs text-neutral-500 opacity-0 transition group-hover:opacity-100 hover:bg-red-500/10 hover:text-red-400"
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
