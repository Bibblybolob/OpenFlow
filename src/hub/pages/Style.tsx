import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";
import { LANGUAGES } from "../../lib/languages";
import type { Style as StyleType } from "../../lib/types";

export default function Style() {
  const [styles, setStyles] = useState<StyleType[]>([]);
  const [appPattern, setAppPattern] = useState("");
  const [label, setLabel] = useState("");
  const [instructions, setInstructions] = useState("");
  const [language, setLanguage] = useState("");

  async function refresh() {
    setStyles(await api.listStyles());
  }

  useEffect(() => {
    refresh();
  }, []);

  async function add() {
    if (!appPattern.trim()) return;
    await api.upsertStyle(
      appPattern,
      label.trim() || appPattern,
      instructions.trim(),
      language || null,
    );
    setAppPattern("");
    setLabel("");
    setInstructions("");
    setLanguage("");
    refresh();
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Style</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Match how FlowClone writes per app. The pattern matches against the
          frontmost app's identifier (e.g. “com.apple.Mail”).
        </p>
      </div>

      <div className="flex flex-col gap-2">
        <div className="flex gap-2">
          <input
            value={appPattern}
            onChange={(e) => setAppPattern(e.target.value)}
            placeholder="App pattern (e.g. mail or slack)"
            className="flex-1 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
          />
          <input
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="Label (optional)"
            className="w-56 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
          />
        </div>
        <textarea
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
          placeholder="Tone instructions (e.g. Formal, concise, no emojis)"
          rows={2}
          className="w-full resize-none rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
        />
        <label className="flex items-center gap-3 rounded-lg border border-white/5 bg-white/[0.03] px-4 py-2.5">
          <span className="w-32 shrink-0 text-xs text-neutral-500">
            Transcribe in
          </span>
          <select
            value={language}
            onChange={(e) => setLanguage(e.target.value)}
            className="min-w-0 flex-1 bg-transparent text-sm text-neutral-200 outline-none [&>option]:bg-[#1a1a20]"
          >
            {LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.code === "" ? "Global setting (auto-detect by default)" : l.label}
              </option>
            ))}
          </select>
        </label>
        <button
          onClick={add}
          className="self-start rounded-lg bg-indigo-500/90 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
        >
          Save style
        </button>
      </div>

      <div className="flex flex-col gap-2">
        {styles.length === 0 && (
          <p className="text-sm text-neutral-600">
            No styles yet. Without one, FlowClone uses a neutral default tone.
          </p>
        )}
        {styles.map((s) => (
          <div
            key={s.id}
            className={`group flex items-start justify-between gap-4 rounded-xl border border-white/5 bg-white/[0.03] px-4 py-3 ${
              s.enabled ? "" : "opacity-50"
            }`}
          >
            <div className="min-w-0">
              <p className="text-sm font-medium text-neutral-200">
                {s.label}{" "}
                <span className="font-normal text-neutral-600">
                  · matches “{s.appPattern}”
                </span>
              </p>
              <p className="mt-1 truncate text-xs text-neutral-500">
                {s.instructions || "No tone instructions"}
              </p>
              {s.language && (
                <p className="text-xs text-indigo-300/80">
                  transcribes in{" "}
                  {LANGUAGES.find((l) => l.code === s.language)?.label ??
                    s.language}
                </p>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button
                onClick={async () => {
                  await api.setStyleEnabled(s.id, !s.enabled);
                  refresh();
                }}
                className="rounded-md px-2 py-1 text-xs text-neutral-400 opacity-0 transition group-hover:opacity-100 hover:bg-white/5"
              >
                {s.enabled ? "Disable" : "Enable"}
              </button>
              <button
                onClick={async () => {
                  await api.deleteStyle(s.id);
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
