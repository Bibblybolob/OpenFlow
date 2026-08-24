import { invoke } from "@tauri-apps/api/core";
import type {
  DictionaryEntry,
  Snippet,
  Stats,
  Style,
  Transcript,
  VocabSuggestion,
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

  listVocabSuggestions: () => invoke<VocabSuggestion[]>("list_vocab_suggestions"),

  acceptVocabSuggestion: (id: number) =>
    invoke<void>("accept_vocab_suggestion", { id }),

  dismissVocabSuggestion: (id: number) =>
    invoke<void>("dismiss_vocab_suggestion", { id }),

  addSnippet: (trigger: string, body: string) =>
    invoke<Snippet>("add_snippet", { trigger, body }),

  listSnippets: () => invoke<Snippet[]>("list_snippets"),

  deleteSnippet: (id: number) => invoke<void>("delete_snippet", { id }),

  upsertStyle: (
    appPattern: string,
    label: string,
    instructions: string,
    language?: string | null,
  ) =>
    invoke<Style>("upsert_style", {
      appPattern,
      label,
      instructions,
      language: language ?? null,
    }),

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

  inputMonitoringStatus: () => invoke<boolean>("input_monitoring_status"),

  hotkeyLastSeen: () =>
    invoke<{ name: string; down: boolean; agoMs: number }[]>(
      "hotkey_last_seen",
    ),

  openAccessibilitySettings: () =>
    invoke<void>("open_accessibility_settings"),

  openInputMonitoringSettings: () =>
    invoke<void>("open_input_monitoring_settings"),

  toggleRecording: () => invoke<string>("toggle_recording"),

  cancelRecording: () => invoke<void>("cancel_recording"),

  pasteText: (text: string) =>
    invoke<void>("paste_text_at_cursor", { text }),

  retryLast: () => invoke<boolean>("retry_last"),

  togglePause: () => invoke<string>("toggle_pause"),

  listMics: () => invoke<string[]>("list_mics"),

  setMicDevice: (name: string | null) =>
    invoke<void>("set_mic_device", { name }),

  pipelineStatus: () => invoke<string>("pipeline_status"),

  micLevel: () =>
    invoke<{ bar: number; voiced: boolean }>("mic_level"),

  hotkeyWatcherStatus: () => invoke<string>("hotkey_watcher_status"),

  getHotkey: () => invoke<string[]>("get_hotkey"),

  hotkeyOptions: () => invoke<string[]>("hotkey_options"),

  setHotkey: (names: string[]) => invoke<string[]>("set_hotkey", { names }),

  autostartStatus: () => invoke<boolean>("autostart_status"),

  setAutostart: (enable: boolean) =>
    invoke<void>("autostart_set", { enable }),

  checkMicPermission: () => invoke<void>("check_mic_permission"),

  setFlowbarVisible: (visible: boolean) =>
    invoke<void>("set_flowbar_visible", { visible }),

  fitFlowbar: (width: number, height: number) =>
    invoke<void>("flowbar_fit", { width, height }),

  setFlowbarPreset: (preset: string) =>
    invoke<void>("set_flowbar_preset", { preset }),

  checkForUpdate: () => invoke<string | null>("check_for_update"),

  installUpdate: () => invoke<void>("install_update"),

  localModelStatus: () =>
    invoke<
      { id: string; label: string; approxMb: number; downloaded: boolean }[]
    >("local_model_status"),

  downloadLocalModel: (model: string) =>
    invoke<void>("download_local_model", { model }),

  setLocalModel: (model: string) =>
    invoke<void>("set_local_model", { model }),
};
