import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

export type PipelineState =
  | "idle"
  | "recording"
  | "transcribing"
  | "injecting"
  | "paused";

export interface PipelineEvent {
  type: PipelineState;
  error?: string;
  transcript?: { id: number; text: string };
}

interface PipelineWarningEvent {
  message: string;
}

export function usePipelineState() {
  const [state, setState] = useState<PipelineState>("idle");
  const [error, setError] = useState<string | null>(null);
  const [warning, setWarning] = useState<string | null>(null);
  const [lastTranscriptId, setLastTranscriptId] = useState<number | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<PipelineEvent>("pipeline", (e) => {
      const ev = e.payload;
      setState(ev.type);
      setError(ev.error ?? null);
      if (ev.transcript) setLastTranscriptId(ev.transcript.id);
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<PipelineWarningEvent>("pipeline-warning", (e) => {
      setWarning(e.payload.message);
    }).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    if (!warning) return;
    const timer = setTimeout(() => setWarning(null), 8000);
    return () => clearTimeout(timer);
  }, [warning]);

  return { state, error, warning, lastTranscriptId };
}
