export interface Transcript {
  id: number;
  text: string;
  rawText: string;
  language: string;
  durationMs: number;
  wordCount: number;
  targetApp: string;
  flagged: boolean;
  createdAt: string;
}

export interface Stats {
  totalWords: number;
  transcriptCount: number;
  streakDays: number;
}

export interface DictionaryEntry {
  id: number;
  term: string;
  replacement: string | null;
  starred: boolean;
  createdAt: string;
}

export interface Snippet {
  id: number;
  trigger: string;
  body: string;
  createdAt: string;
}

export interface Style {
  id: number;
  appPattern: string;
  label: string;
  instructions: string;
  language?: string | null;
  enabled: boolean;
  createdAt: string;
}

export interface VocabSuggestion {
  id: number;
  rawForm: string;
  term: string;
  occurrences: number;
  createdAt: string;
}
