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

export function usePipelineState() {
  const [state, setState] = useState<PipelineState>("idle");
  const [error, setError] = useState<string | null>(null);
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

  return { state, error, lastTranscriptId };
}
