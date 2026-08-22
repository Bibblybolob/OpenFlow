import { invoke } from "@tauri-apps/api/core";
import type {
  DictionaryEntry,
  Snippet,
  Stats,
  Style,
  Transcript,
} from "./types";

export const api = {
  insertTranscript: (t: {
    text: string;
    rawText: string;
    language: string;
    durationMs: number;
    targetApp: string;
  }) =>
    invoke<Transcript>("insert_transcript", {
      text: t.text,
      rawText: t.rawText,
      language: t.language,
      durationMs: t.durationMs,
      targetApp: t.targetApp,
    }),

  listTranscripts: (limit = 100, offset = 0) =>
    invoke<Transcript[]>("list_transcripts", { limit, offset }),

  searchTranscripts: (query: string) =>
    invoke<Transcript[]>("search_transcripts", { query }),

  deleteTranscript: (id: number) => invoke<void>("delete_transcript", { id }),

  setFlagged: (id: number, flagged: boolean) =>
    invoke<void>("set_flagged", { id, flagged }),

  stats: () => invoke<Stats>("transcript_stats"),

  addDictionaryTerm: (term: string, replacement?: string) =>
    invoke<DictionaryEntry>("add_dictionary_term", {
      term,
      replacement: replacement ?? null,
    }),

  listDictionary: () => invoke<DictionaryEntry[]>("list_dictionary"),

  setDictionaryStarred: (id: number, starred: boolean) =>
    invoke<void>("set_dictionary_starred", { id, starred }),

  deleteDictionaryTerm: (id: number) =>
    invoke<void>("delete_dictionary_term", { id }),

  addSnippet: (trigger: string, body: string) =>
    invoke<Snippet>("add_snippet", { trigger, body }),

  listSnippets: () => invoke<Snippet[]>("list_snippets"),

  deleteSnippet: (id: number) => invoke<void>("delete_snippet", { id }),

  upsertStyle: (appPattern: string, label: string, instructions: string) =>
    invoke<Style>("upsert_style", { appPattern, label, instructions }),

  listStyles: () => invoke<Style[]>("list_styles"),

  setStyleEnabled: (id: number, enabled: boolean) =>
    invoke<void>("set_style_enabled", { id, enabled }),

  deleteStyle: (id: number) => invoke<void>("delete_style", { id }),

  resolveStyle: (appIdentifier: string) =>
    invoke<string | null>("resolve_style", { appIdentifier }),

  getSetting: <T>(key: string) => invoke<T | null>("get_setting", { key }),

  setSetting: (key: string, value: unknown) =>
    invoke<void>("set_setting", { key, value }),

  accessibilityStatus: () => invoke<boolean>("accessibility_status"),

  openAccessibilitySettings: () =>
    invoke<void>("open_accessibility_settings"),

  toggleRecording: () => invoke<string>("toggle_recording"),

  cancelRecording: () => invoke<void>("cancel_recording"),

  getHotkey: () => invoke<string[]>("get_hotkey"),

  setHotkey: (names: string[]) => invoke<string[]>("set_hotkey", { names }),

  autostartStatus: () => invoke<boolean>("autostart_status"),

  setAutostart: (enable: boolean) =>
    invoke<void>("autostart_set", { enable }),
};
