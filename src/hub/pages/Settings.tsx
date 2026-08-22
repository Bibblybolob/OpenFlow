import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";

type ProviderChoice = "auto" | "openai" | "anthropic";

const LANGUAGES: { code: string; label: string }[] = [
  { code: "auto", label: "Auto-detect" },
  { code: "en", label: "English" },
  { code: "es", label: "Español" },
  { code: "fr", label: "Français" },
  { code: "de", label: "Deutsch" },
  { code: "pt", label: "Português" },
  { code: "it", label: "Italiano" },
  { code: "nl", label: "Nederlands" },
  { code: "pl", label: "Polski" },
  { code: "tr", label: "Türkçe" },
  { code: "ru", label: "Русский" },
  { code: "uk", label: "Українська" },
  { code: "ar", label: "العربية" },
  { code: "hi", label: "हिन्दी" },
  { code: "zh", label: "中文" },
  { code: "ja", label: "日本語" },
  { code: "ko", label: "한국어" },
  { code: "vi", label: "Tiếng Việt" },
  { code: "th", label: "ไทย" },
];

const HOTKEY_CHOICES = [
  "F1",
  "F2",
  "F3",
  "F4",
  "F5",
  "F6",
  "F7",
  "F8",
  "F9",
  "F10",
  "F11",
  "F12",
  "CapsLock",
  "Right Shift",
  "Right Ctrl",
  "Right Alt",
  "Right Cmd",
];

export default function Settings() {
  const [openaiKey, setOpenaiKey] = useState("");
  const [anthropicKey, setAnthropicKey] = useState("");
  const [savedOpenai, setSavedOpenai] = useState(false);
  const [savedAnthropic, setSavedAnthropic] = useState(false);
  const [provider, setProvider] = useState<ProviderChoice>("auto");
  const [model, setModel] = useState("");
  const [language, setLanguage] = useState("auto");
  const [hotkey, setHotkey] = useState<string[]>(["F5"]);
  const [autostart, setAutostart] = useState(false);
  const [commandMode, setCommandMode] = useState(true);
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
    const lang = (await api.getSetting<string>("language")) ?? "auto";
    const hk = await api.getHotkey().catch(() => ["F5"]);
    const as = await api.autostartStatus().catch(() => false);
    const cm = await api.getSetting<boolean>("commandMode");

    setSavedOpenai(Boolean(ok));
    setSavedAnthropic(Boolean(ak));
    setOpenaiKey(maskKey(ok));
    setAnthropicKey(maskKey(ak));
    setProvider(prov ?? "auto");
    setModel(mdl ?? "");
    setLanguage(lang ?? "auto");
    setHotkey(hk.length ? hk : ["F5"]);
    setAutostart(as);
    setCommandMode(cm ?? true);
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
    await api.setSetting("llmProvider", provider);
    await api.setSetting("llmModel", model.trim() || "");
    refresh();
  }

  async function changeHotkey(name: string) {
    setHotkey([name]);
    try {
      const applied = await api.setHotkey([name]);
      setHotkey(applied);
    } catch (e) {
      console.error(e);
    }
  }

  async function changeLanguage(code: string) {
    setLanguage(code);
    await api.setSetting("language", code === "auto" ? "" : code);
  }

  async function toggleAutostart() {
    const next = !autostart;
    setAutostart(next);
    try {
      await api.setAutostart(next);
    } catch (e) {
      console.error(e);
      setAutostart(!next);
    }
  }

  async function toggleCommandMode() {
    const next = !commandMode;
    setCommandMode(next);
    try {
      await api.setSetting("commandMode", next);
    } catch (e) {
      console.error(e);
      setCommandMode(!next);
    }
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
          Dictation
        </h2>
        <div className="flex gap-2">
          <SelectRow
            label="Hotkey"
            value={hotkey.join(" + ")}
            options={HOTKEY_CHOICES.map((k) => ({ value: k, label: k }))}
            onChange={(v) => changeHotkey(v)}
          />
          <SelectRow
            label="Language"
            value={language || "auto"}
            options={LANGUAGES.map((l) => ({ value: l.code, label: l.label }))}
            onChange={changeLanguage}
          />
        </div>
        <ToggleRow
          label="Launch at login"
          hint="Start FlowClone automatically when you sign in"
          checked={autostart}
          onChange={toggleAutostart}
        />
        <ToggleRow
          label="Voice commands"
          hint='Say things like "open youtube", "search rust async", or "copy …" to act instead of typing'
          checked={commandMode}
          onChange={toggleCommandMode}
        />
        <p className="text-xs text-neutral-600">
          Hold the hotkey to dictate. Quick double-tap switches to hands-free
          mode; tap again or press Esc to stop.
        </p>
      </section>

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
    </div>
  );
}

function SelectRow({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (v: string) => void;
}) {
  return (
    <label className="flex flex-1 items-center gap-3 rounded-lg border border-white/5 bg-white/[0.03] px-4 py-2.5">
      <span className="w-16 shrink-0 text-xs text-neutral-500">{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="min-w-0 flex-1 bg-transparent text-sm text-neutral-200 outline-none [&>option]:bg-[#1a1a20]"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function ToggleRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <div className="flex items-center justify-between rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
      <div>
        <p className="text-sm text-neutral-300">{label}</p>
        <p className="text-[11px] text-neutral-600">{hint}</p>
      </div>
      <button
        role="switch"
        aria-checked={checked}
        onClick={onChange}
        className={`relative h-6 w-11 shrink-0 rounded-full transition-colors ${
          checked ? "bg-indigo-500" : "bg-white/10"
        }`}
      >
        <span
          className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${
            checked ? "left-[22px]" : "left-0.5"
          }`}
        />
      </button>
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
  const dirty = Boolean(value) && !value.includes("•");
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
      <button
        onClick={onSave}
        disabled={!dirty}
        className={`shrink-0 rounded-lg px-4 py-2 text-sm font-medium transition ${
          dirty
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
