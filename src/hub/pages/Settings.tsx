import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";

const DEFAULTS: { key: string; label: string; value: string }[] = [
  { key: "pushToTalk", label: "Push to talk", value: "Hold F5" },
  { key: "cancel", label: "Cancel dictation", value: "Esc" },
];

export default function Settings() {
  const [apiKey, setApiKey] = useState("");
  const [savedKey, setSavedKey] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [accessibility, setAccessibility] = useState<boolean | null>(null);

  async function refresh() {
    const stored = await api.getSetting<string>("openaiApiKey");
    setSavedKey(stored ?? null);
    setApiKey(stored ? maskKey(stored) : "");
    setAccessibility(await invokeAccessibility());
  }

  useEffect(() => {
    refresh();
  }, []);

  async function save() {
    const trimmed = apiKey.trim();
    if (!trimmed || trimmed === maskKey(savedKey ?? "")) return;
    setSaving(true);
    await api.setSetting("openaiApiKey", trimmed);
    setSaving(false);
    refresh();
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Settings</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Shortcuts, transcription, and privacy configuration.
        </p>
      </div>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Transcription
        </h2>
        <div className="flex items-center gap-2">
          <input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="OpenAI API key (sk-…)"
            className="flex-1 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
          />
          <button
            onClick={save}
            disabled={saving}
            className="rounded-lg bg-indigo-500/90 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500 disabled:opacity-50"
          >
            Save
          </button>
        </div>
        <p className="text-xs text-neutral-600">
          Stored locally on this device. The OPENAI_API_KEY environment
          variable takes precedence when set.
          {savedKey && " A key is currently saved."}
        </p>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Permissions
        </h2>
        <div className="overflow-hidden rounded-xl border border-white/5">
          <PermRow label="Microphone" ok={true} hint="Granted on first use" />
          <PermRow
            label="Accessibility (typing into apps)"
            ok={accessibility === true}
            hint={
              accessibility === false
                ? "Required to paste text"
                : "Needed to paste text at your cursor"
            }
            action={
              accessibility === false ? (
                <button
                  onClick={openAccessibilitySettings}
                  className="rounded-md bg-indigo-500/20 px-2 py-1 text-xs text-indigo-300 transition hover:bg-indigo-500/30"
                >
                  Open System Settings
                </button>
              ) : null
            }
          />
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Shortcuts
        </h2>
        <div className="overflow-hidden rounded-xl border border-white/5">
          {DEFAULTS.map((d) => (
            <div
              key={d.key}
              className="flex items-center justify-between bg-white/[0.03] px-4 py-3 not-last:border-b not-last:border-white/5"
            >
              <span className="text-sm text-neutral-300">{d.label}</span>
              <span className="rounded-md border border-white/10 bg-white/[0.05] px-2 py-0.5 text-xs text-neutral-400">
                {d.value}
              </span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}

function PermRow({
  label,
  ok,
  hint,
  action,
}: {
  label: string;
  ok: boolean;
  hint: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between bg-white/[0.03] px-4 py-3 not-last:border-b not-last:border-white/5">
      <div>
        <span className="text-sm text-neutral-300">{label}</span>
        {!ok && <p className="text-xs text-neutral-600">{hint}</p>}
      </div>
      {action ?? (
        <span
          className={`text-xs ${ok ? "text-emerald-400" : "text-amber-400"}`}
        >
          {ok ? "Granted" : "Check"}
        </span>
      )}
    </div>
  );
}

function maskKey(key: string | null): string {
  if (!key) return "";
  if (key.length <= 8) return "••••••••";
  return `${key.slice(0, 4)}${"•".repeat(Math.max(key.length - 8, 4))}${key.slice(-4)}`;
}

async function invokeAccessibility(): Promise<boolean> {
  try {
    return await api.accessibilityStatus();
  } catch {
    return true;
  }
}

async function openAccessibilitySettings() {
  try {
    await api.openAccessibilitySettings();
  } catch {
    // best effort only
  }
}
