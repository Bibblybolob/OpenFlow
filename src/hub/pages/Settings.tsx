import { api } from "../../lib/ipc";

const DEFAULTS: { key: string; label: string; value: string }[] = [
  { key: "pushToTalk", label: "Push to talk", value: "Hold Fn (customizable)" },
  {
    key: "handsFree",
    label: "Hands-free toggle",
    value: "Double-tap Fn",
  },
  { key: "cancel", label: "Cancel dictation", value: "Esc" },
];

export default function Settings() {
  return (
    <div className="flex h-full flex-col gap-6 overflow-y-auto p-8">
      <div>
        <h1 className="text-xl font-semibold">Settings</h1>
        <p className="mt-1 text-sm text-neutral-500">
          Shortcut, microphone, language, and privacy configuration lands here
          in Milestone 5.
        </p>
      </div>

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium uppercase tracking-wider text-neutral-500">
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

      <section className="flex flex-col gap-2">
        <h2 className="text-xs font-medium uppercase tracking-wider text-neutral-500">
          Data &amp; Privacy
        </h2>
        <button
          onClick={() => api.setSetting("privacyMode", true)}
          className="self-start rounded-lg border border-white/10 px-4 py-2 text-sm text-neutral-400 transition hover:bg-white/5"
        >
          Enable privacy mode (no transcript history)
        </button>
      </section>
    </div>
  );
}
