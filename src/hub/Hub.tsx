import { useState, type ReactNode } from "react";
import Sidebar, { type Page } from "./components/Sidebar";
import { usePipelineState } from "./usePipelineState";
import Home from "./pages/Home";
import Dictionary from "./pages/Dictionary";
import Snippets from "./pages/Snippets";
import Style from "./pages/Style";
import Settings from "./pages/Settings";

const LABELS: Record<string, string> = {
  idle: "Ready · hold F5 to talk",
  recording: "Recording…",
  transcribing: "Transcribing…",
  injecting: "Pasting…",
};

export default function Hub() {
  const [page, setPage] = useState<Page>("home");
  const { state, error, lastTranscriptId } = usePipelineState();

  let statusContent: ReactNode = LABELS[state];
  if (error) statusContent = <span className="text-red-400">{error}</span>;

  return (
    <div className="relative flex h-screen overflow-hidden">
      <Sidebar page={page} onNavigate={setPage} />
      <main className="min-w-0 flex-1">
        {page === "home" && <Home key={lastTranscriptId ?? "none"} />}
        {page === "dictionary" && <Dictionary />}
        {page === "snippets" && <Snippets />}
        {page === "style" && <Style />}
        {page === "settings" && <Settings />}
      </main>
      <div
        className={`pointer-events-none absolute right-6 bottom-5 rounded-full px-4 py-1.5 text-xs shadow-lg backdrop-blur transition-colors ${
          error
            ? "bg-red-500/15 text-red-300"
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
