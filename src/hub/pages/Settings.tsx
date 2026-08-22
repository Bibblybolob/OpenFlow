import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";

type ProviderChoice = "auto" | "openai" | "anthropic";

const DEFAULTS: { key: string; label: string; value: string }[] = [
  { key: "pushToTalk", label: "Push to talk", value: "Hold F5" },
  { key: "cancel", label: "Cancel dictation", value: "Esc" },
];

export default function Settings() {
  const [openaiKey, setOpenaiKey] = useState("");
  const [anthropicKey, setAnthropicKey] = useState("");
  const [savedOpenai, setSavedOpenai] = useState(false);
  const [savedAnthropic, setSavedAnthropic] = useState(false);
  const [provider, setProvider] = useState<ProviderChoice>("auto");
  const [model, setModel] = useState("");
  const [accessibility, setAccessibility] = useState<boolean | null>(null);

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function refresh() {
    const ok = await api.getSetting<string>("openaiApiKey");
    const ak = await api.getSetting<string>("anthropicApiKey");
    const prov =
      ((await api.getSetting<string>("llmProvider")) as ProviderChoice) ?? "auto";
    const mdl = await api.getSetting<string>("llmModel");

    setSavedOpenai(Boolean(ok));
    setSavedAnthropic(Boolean(ak));
    setOpenaiKey(maskKey(ok));
    setAnthropicKey(maskKey(ak));
    setProvider(prov ?? "auto");
    setModel(mdl ?? "");
    setAccessibility(await invokeAccessibility());
  }

  async function saveKey(kind: "openai" | "anthropic") {
    if (kind === "openai") {
      const value = openaiKey.trim();
      if (!value || value.includes("•")) return;
      await api.setSetting("openaiApiKey", value);
    } else {
      const value = anthropicKey.trim();
      if (!value || value.includes("•")) return;
      await api.setSetting("anthropicApiKey", value);
    }
    refresh();
  }

  async function saveCleanup() {
    await api.setSetting(
      "llmProvider",
      provider === "auto" ? "auto" : provider,
    );
    await api.setSetting("llmModel", model.trim() || "");
    refresh();
  }

  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Settings</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Transcription, cleanup LLM, permissions, and shortcuts.
        </p>
      </div>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          API keys
        </h2>
        <KeyRow
          label="OpenAI"
          hint="Used for transcription; also available for cleanup"
          saved={savedOpenai}
          value={openaiKey}
          onChange={setOpenaiKey}
          onSave={() => saveKey("openai")}
          placeholder="sk-…"
        />
        <KeyRow
          label="Claude (Anthropic)"
          hint="Optional alternative for the cleanup step"
          saved={savedAnthropic}
          value={anthropicKey}
          onChange={setAnthropicKey}
          onSave={() => saveKey("anthropic")}
          placeholder="sk-ant-…"
        />
        <p className="text-xs text-neutral-600">
          Keys are stored locally on this device. Environment variables
          OPENAI_API_KEY / ANTHROPIC_API_KEY take precedence when set.
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Cleanup
        </h2>
        <div className="flex gap-2">
          <select
            value={provider}
            onChange={(e) => setProvider(e.target.value as ProviderChoice)}
            className="w-48 rounded-lg border border-white/10 bg-white/[0.04] px-3 py-2 text-sm outline-none focus:border-indigo-400/60"
          >
            <option value="auto">Auto-detect</option>
            <option value="openai">OpenAI</option>
            <option value="anthropic">Claude</option>
          </select>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={
              provider === "anthropic"
                ? "claude-3-5-haiku-latest (default)"
                : provider === "openai"
                  ? "gpt-4o-mini (default)"
                  : "Model override (optional)"
            }
            className="flex-1 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
          />
          <button
            onClick={saveCleanup}
            className="rounded-lg bg-indigo-500/90 px-4 py-2 text-sm font-medium text-white transition hover:bg-indigo-500"
          >
            Save
          </button>
        </div>
        <p className="text-xs text-neutral-600">
          Auto-detect prefers OpenAI when both keys exist. If cleanup fails,
          your raw dictation is pasted anyway so nothing is ever lost.
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
                ? "Required to paste text and detect the frontmost app"
                : "Needed to paste at your cursor and match app styles"
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

function KeyRow({
  label,
  hint,
  saved,
  value,
  onChange,
  onSave,
  placeholder,
}: {
  label: string;
  hint: string;
  saved: boolean;
  value: string;
  onChange: (v: string) => void;
  onSave: () => void;
  placeholder: string;
}) {
  return (
    <div className="flex items-center gap-3">
      <div className="w-40 shrink-0">
        <p className="text-sm text-neutral-300">{label}</p>
        <p className="text-[11px] leading-tight text-neutral-600">{hint}</p>
      </div>
      <input
        type="password"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="min-w-0 flex-1 rounded-lg border border-white/10 bg-white/[0.04] px-4 py-2 text-sm outline-none placeholder:text-neutral-600 focus:border-indigo-400/60"
      />
      {saved && !value.includes("•") && null}
      <button
        onClick={onSave}
        className={`shrink-0 rounded-lg px-4 py-2 text-sm font-medium transition ${
          value && !value.includes("•")
            ? "bg-indigo-500/90 text-white hover:bg-indigo-500"
            : "border border-white/10 text-neutral-500"
        }`}
      >
        {saved ? "Update" : "Save"}
      </button>
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
