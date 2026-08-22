import { useEffect, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { api } from "../../lib/ipc";
import {
  ACCENTS,
  DEFAULT_PILL_STYLE,
  PILL_STYLE_KEY,
  accentOf,
  loadPillStyle,
  pillBackground,
  pillRadius,
  rgba,
  shade,
  type PillAccent,
  type PillShape,
  type PillStyle,
} from "../../lib/pillStyle";

type ProviderChoice = "auto" | "openai" | "anthropic" | "openrouter";

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

export default function Settings({
  onRerunSetup,
}: {
  onRerunSetup: () => void;
}) {
  const [openaiKey, setOpenaiKey] = useState("");
  const [anthropicKey, setAnthropicKey] = useState("");
  const [openrouterKey, setOpenrouterKey] = useState("");
  const [savedOpenai, setSavedOpenai] = useState(false);
  const [savedAnthropic, setSavedAnthropic] = useState(false);
  const [savedOpenrouter, setSavedOpenrouter] = useState(false);
  const [flowbarPreset, setFlowbarPreset] = useState("bottom_center");
  const [pillStyle, setPillStyle] = useState<PillStyle>(DEFAULT_PILL_STYLE);
  const [provider, setProvider] = useState<ProviderChoice>("auto");
  const [model, setModel] = useState("");
  const [language, setLanguage] = useState("auto");
  const [hotkey, setHotkey] = useState<string[]>(["F5"]);
  const [autostart, setAutostart] = useState(false);
  const [commandMode, setCommandMode] = useState(true);
  const [accessibility, setAccessibility] = useState<boolean | null>(null);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function refresh() {
    const ok = await api.getSetting<string>("openaiApiKey");
    const ak = await api.getSetting<string>("anthropicApiKey");
    const orkey = await api.getSetting<string>("openrouterApiKey");
    const prov =
      ((await api.getSetting<string>("llmProvider")) as ProviderChoice) ?? "auto";
    const mdl = await api.getSetting<string>("llmModel");
    const lang = (await api.getSetting<string>("language")) ?? "auto";
    const hk = await api.getHotkey().catch(() => ["F5"]);
    const as = await api.autostartStatus().catch(() => false);
    const cm = await api.getSetting<boolean>("commandMode");
    const style = await loadPillStyle();

    setSavedOpenai(Boolean(ok));
    setSavedAnthropic(Boolean(ak));
    setSavedOpenrouter(Boolean(orkey));
    setOpenaiKey(maskKey(ok));
    setAnthropicKey(maskKey(ak));
    setOpenrouterKey(maskKey(orkey));
    setProvider(prov ?? "auto");
    setModel(mdl ?? "");
    setLanguage(lang ?? "auto");
    setHotkey(hk.length ? hk : ["F5"]);
    setAutostart(as);
    setCommandMode(cm ?? true);
    setPillStyle(style);
    setAccessibility(await invokeAccessibility());
  }

  async function saveKey(kind: "openai" | "anthropic" | "openrouter") {
    const value = (kind === "openai"
      ? openaiKey
      : kind === "anthropic"
        ? anthropicKey
        : openrouterKey
    ).trim();
    if (!value || value.includes("•")) return;
    await api.setSetting(`${kind}ApiKey`, value);
    refresh();
  }

  async function changeFlowbarPreset(preset: string) {
    setFlowbarPreset(preset);
    try {
      await api.setFlowbarPreset(preset);
    } catch (e) {
      console.error(e);
    }
  }

  async function changePillStyle(patch: Partial<PillStyle>) {
    const next = { ...pillStyle, ...patch };
    setPillStyle(next);
    try {
      await api.setSetting(PILL_STYLE_KEY, next);
      await emit("flowbar-style-changed");
    } catch (e) {
      console.error(e);
    }
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

  async function checkForUpdate() {
    setUpdateStatus("Checking…");
    try {
      const version = await api.checkForUpdate();
      if (version) {
        setUpdateStatus(`Version ${version} available — installing…`);
        await api.installUpdate();
        setUpdateStatus("Installed. Restarting…");
      } else {
        setUpdateStatus("You're up to date.");
      }
    } catch (e) {
      console.error(e);
      setUpdateStatus(String(e).replace(/^.*failed: /, "Check failed: "));
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

      <button
        onClick={onRerunSetup}
        className="self-start rounded-lg border border-white/10 px-4 py-2 text-xs text-neutral-400 transition hover:bg-white/5"
      >
        Run setup wizard again
      </button>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          About
        </h2>
        <div className="flex items-center justify-between rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <span className="text-sm text-neutral-300">
            Version {import.meta.env.PACKAGE_VERSION ?? "0.1.0"}
          </span>
          <div className="flex items-center gap-3">
            {updateStatus && (
              <span className="text-xs text-neutral-500">{updateStatus}</span>
            )}
            <button
              onClick={checkForUpdate}
              className="rounded-md border border-white/10 px-2.5 py-1 text-xs text-neutral-300 transition hover:bg-white/5"
            >
              Check for updates
            </button>
          </div>
        </div>
      </section>

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

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Flow Bar
        </h2>
        <SelectRow
          label="Position"
          value={flowbarPreset}
          options={[
            { value: "top_left", label: "Top left" },
            { value: "top_center", label: "Top center" },
            { value: "top_right", label: "Top right" },
            { value: "bottom_left", label: "Bottom left" },
            { value: "bottom_center", label: "Bottom center" },
            { value: "bottom_right", label: "Bottom right" },
          ]}
          onChange={changeFlowbarPreset}
        />
        <p className="text-xs text-neutral-600">
          You can also grab the pill and drag it anywhere — its spot is
          remembered across restarts.
        </p>
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-xs font-medium tracking-wider text-neutral-500 uppercase">
          Pill appearance
        </h2>
        <div className="flex items-center justify-center rounded-xl border border-white/5 bg-white/[0.02] py-8">
          <PillPreview style={pillStyle} />
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <p className="mb-2 text-sm text-neutral-300">Shape</p>
          <div className="flex gap-2">
            {(
              [
                ["pill", "Pill"],
                ["rounded", "Rounded"],
                ["square", "Square"],
              ] as [PillShape, string][]
            ).map(([value, label]) => (
              <button
                key={value}
                onClick={() => changePillStyle({ shape: value })}
                className={`flex-1 rounded-lg border px-3 py-2 text-xs transition ${
                  pillStyle.shape === value
                    ? "border-indigo-400/60 bg-indigo-500/15 text-white"
                    : "border-white/10 text-neutral-400 hover:bg-white/5"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <p className="mb-2 text-sm text-neutral-300">Accent</p>
          <div className="flex gap-2">
            {(Object.keys(ACCENTS) as PillAccent[]).map((key) => (
              <button
                key={key}
                title={ACCENTS[key].label}
                onClick={() => changePillStyle({ accent: key })}
                className={`h-7 w-7 rounded-full border-2 transition ${
                  pillStyle.accent === key
                    ? "scale-110 border-white"
                    : "border-transparent hover:scale-105"
                }`}
                style={{ backgroundColor: ACCENTS[key].swatch }}
              />
            ))}
          </div>
        </div>
        <div className="rounded-lg border border-white/5 bg-white/[0.03] px-4 py-3">
          <div className="mb-2 flex items-center justify-between">
            <p className="text-sm text-neutral-300">Opacity</p>
            <span className="text-xs text-neutral-500">{pillStyle.opacity}%</span>
          </div>
          <input
            type="range"
            min={40}
            max={100}
            step={1}
            value={pillStyle.opacity}
            onChange={(e) =>
              setPillStyle({ ...pillStyle, opacity: Number(e.target.value) })
            }
            onMouseUp={(e) =>
              changePillStyle({ opacity: Number((e.target as HTMLInputElement).value) })
            }
            onTouchEnd={(e) =>
              changePillStyle({ opacity: Number((e.target as HTMLInputElement).value) })
            }
            onKeyUp={(e) =>
              changePillStyle({ opacity: Number((e.target as HTMLInputElement).value) })
            }
            className="w-full accent-indigo-400"
          />
        </div>
        <ToggleRow
          label="Animations"
          hint="Pop-in motion, glow while recording, button feedback"
          checked={pillStyle.animations}
          onChange={() => changePillStyle({ animations: !pillStyle.animations })}
        />
        <ToggleRow
          label="Hide when idle"
          hint='The pill only appears while dictating; bring it back with the hotkey'
          checked={pillStyle.autoHide}
          onChange={() => changePillStyle({ autoHide: !pillStyle.autoHide })}
        />
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
        <KeyRow
          label="OpenRouter"
          hint="One key, hundreds of cleanup models"
          saved={savedOpenrouter}
          value={openrouterKey}
          onChange={setOpenrouterKey}
          onSave={() => saveKey("openrouter")}
          placeholder="sk-or-…"
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
            <option value="openrouter">OpenRouter</option>
          </select>
          <input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder={
              provider === "anthropic"
                ? "claude-3-5-haiku-latest (default)"
                : provider === "openai"
                  ? "gpt-4o-mini (default)"
                  : provider === "openrouter"
                    ? "anthropic/claude-3.5-haiku (default)"
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

function PillPreview({ style }: { style: PillStyle }) {
  const accent = accentOf(style);
  const radius = pillRadius(style.shape);
  return (
    <div
      className={`flex items-center gap-3 border px-4 py-2 shadow-2xl ${radius}`}
      style={{
        background: pillBackground("active", style.opacity),
        borderColor: rgba(accent.border, 0.55),
      }}
    >
      <span
        className="flex h-9 w-9 items-center justify-center rounded-full text-white"
        style={{
          background: `linear-gradient(135deg, ${accent.base}, ${shade(accent.base, -0.4)})`,
        }}
      >
        <svg viewBox="0 0 24 24" fill="currentColor" className="h-4 w-4">
          <path d="M12 15a3.5 3.5 0 0 0 3.5-3.5V6a3.5 3.5 0 1 0-7 0v5.5A3.5 3.5 0 0 0 12 15Z" />
          <path d="M18.5 11.5a.75.75 0 0 0-1.5 0 5 5 0 0 1-10 0 .75.75 0 0 0-1.5 0 6.5 6.5 0 0 0 5.75 6.46V21h-2.25a.75.75 0 0 0 0 1.5h6a.75.75 0 0 0 0-1.5H12.75v-3.04a6.5 6.5 0 0 0 5.75-6.46Z" />
        </svg>
      </span>
      <div className="flex h-8 w-24 items-center justify-center gap-1">
        {[40, 65, 100, 78, 52].map((h, i) => (
          <span
            key={i}
            className="w-1 rounded-full"
            style={{ height: `${h}%`, backgroundColor: accent.soft }}
          />
        ))}
      </div>
      <span
        className="h-2 w-2 rounded-full"
        style={{ backgroundColor: rgba(accent.base, 0.85) }}
      />
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
