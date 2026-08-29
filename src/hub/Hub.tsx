import { useEffect, useState, type ReactNode } from "react";
import Sidebar, { type Page } from "./components/Sidebar";
import Onboarding from "./pages/Onboarding";
import Settings from "./pages/Settings";
import Home from "./pages/Home";
import Dictionary from "./pages/Dictionary";
import Snippets from "./pages/Snippets";
import Style from "./pages/Style";
import { api } from "../lib/ipc";
import { usePipelineState } from "./usePipelineState";

const LABELS: Record<string, string> = {
  idle: "Ready · use hotkey to talk",
  recording: "Recording…",
  transcribing: "Transcribing…",
  injecting: "Pasting…",
};

export default function Hub() {
  const [page, setPage] = useState<Page>("home");
  const [onboarded, setOnboarded] = useState<boolean | null>(null);
  const { state, error, warning, lastTranscriptId } = usePipelineState();

  useEffect(() => {
    api
      .getSetting<boolean>("onboardingComplete")
      .then((v) => setOnboarded(v ?? false))
      .catch(() => setOnboarded(false));
  }, []);

  if (onboarded === null) {
    return <div className="h-screen bg-[#0d0d10]" />;
  }

  if (!onboarded) {
    return (
      <Onboarding
        pipelineState={state}
        lastTranscriptId={lastTranscriptId}
        onComplete={() => {
          setOnboarded(true);
          setPage("home");
        }}
      />
    );
  }

  let statusContent: ReactNode = LABELS[state];
  if (error) statusContent = <span className="text-red-400">{error}</span>;
  else if (warning) statusContent = <span className="text-amber-300">{warning}</span>;

  return (
    <div className="relative flex h-screen overflow-hidden">
      <Sidebar page={page} onNavigate={setPage} />
      <main className="min-w-0 flex-1">
        {page === "home" && <Home key={lastTranscriptId ?? "none"} />}
        {page === "dictionary" && <Dictionary />}
        {page === "snippets" && <Snippets />}
        {page === "style" && <Style />}
        {page === "settings" && (
          <Settings
            onRerunSetup={async () => {
              await api.setFlowbarVisible(false);
              await api.setSetting("onboardingComplete", false);
              setOnboarded(false);
            }}
          />
        )}
      </main>
      <div
        className={`pointer-events-none absolute right-6 bottom-5 rounded-full px-4 py-1.5 text-xs shadow-lg backdrop-blur transition-colors ${
          error
            ? "bg-red-500/15 text-red-300"
            : warning
              ? "bg-amber-500/15 text-amber-200"
            : state !== "idle"
              ? "bg-indigo-500/25 text-indigo-200"
              : "bg-white/[0.06] text-neutral-400"
        }`}
      >
        <span className="flex items-center gap-2">
          {state === "recording" && (
            <span className="h-2 w-2 animate-pulse rounded-full bg-red-400" />
          )}
          {statusContent}
        </span>
      </div>
    </div>
  );
}
