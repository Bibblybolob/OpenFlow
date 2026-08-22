import { api } from "./ipc";

export type PillShape = "pill" | "rounded" | "square";

export type PillAccent =
  | "indigo"
  | "emerald"
  | "rose"
  | "amber"
  | "sky"
  | "graphite";

export interface PillStyle {
  shape: PillShape;
  accent: PillAccent;
  opacity: number;
  animations: boolean;
  autoHide: boolean;
}

export const PILL_STYLE_KEY = "flowBarStyle";

export const DEFAULT_PILL_STYLE: PillStyle = {
  shape: "pill",
  accent: "indigo",
  opacity: 88,
  animations: true,
  autoHide: true,
};

export interface AccentTheme {
  label: string;
  swatch: string;
  base: string;
  soft: string;
  border: string;
  glow: string;
}

export const ACCENTS: Record<PillAccent, AccentTheme> = {
  indigo: {
    label: "Indigo",
    swatch: "#818cf8",
    base: "#818cf8",
    soft: "#c7d2fe",
    border: "#6366f1",
    glow: "99,102,241",
  },
  emerald: {
    label: "Emerald",
    swatch: "#34d399",
    base: "#34d399",
    soft: "#a7f3d0",
    border: "#10b981",
    glow: "16,185,129",
  },
  rose: {
    label: "Rose",
    swatch: "#fb7185",
    base: "#fb7185",
    soft: "#fecdd3",
    border: "#f43f5e",
    glow: "244,63,94",
  },
  amber: {
    label: "Amber",
    swatch: "#fbbf24",
    base: "#fbbf24",
    soft: "#fde68a",
    border: "#f59e0b",
    glow: "245,158,11",
  },
  sky: {
    label: "Sky",
    swatch: "#38bdf8",
    base: "#38bdf8",
    soft: "#bae6fd",
    border: "#0ea5e9",
    glow: "14,165,233",
  },
  graphite: {
    label: "Graphite",
    swatch: "#a3a3a3",
    base: "#a3a3a3",
    soft: "#e5e5e5",
    border: "#737373",
    glow: "163,163,163",
  },
};

export function accentOf(style: PillStyle): AccentTheme {
  return ACCENTS[style.accent] ?? ACCENTS.indigo;
}

export function normalizePillStyle(raw: unknown): PillStyle {
  const obj = (raw ?? {}) as Partial<PillStyle>;
  return {
    shape: (
      ["pill", "rounded", "square"] as PillShape[]
    ).includes(obj.shape as PillShape)
      ? (obj.shape as PillShape)
      : DEFAULT_PILL_STYLE.shape,
    accent: obj.accent && obj.accent in ACCENTS
      ? (obj.accent as PillAccent)
      : DEFAULT_PILL_STYLE.accent,
    opacity:
      typeof obj.opacity === "number" && obj.opacity >= 40 && obj.opacity <= 100
        ? Math.round(obj.opacity)
        : DEFAULT_PILL_STYLE.opacity,
    animations:
      typeof obj.animations === "boolean"
        ? obj.animations
        : DEFAULT_PILL_STYLE.animations,
    autoHide:
      typeof obj.autoHide === "boolean"
        ? obj.autoHide
        : DEFAULT_PILL_STYLE.autoHide,
  };
}

export async function loadPillStyle(): Promise<PillStyle> {
  try {
    return normalizePillStyle(await api.getSetting(PILL_STYLE_KEY));
  } catch {
    return { ...DEFAULT_PILL_STYLE };
  }
}

export function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function rgba(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r},${g},${b},${alpha})`;
}

export function shade(hex: string, factor: number): string {
  const [r, g, b] = hexToRgb(hex);
  const f = (c: number) =>
    Math.round(factor < 0 ? c * (1 + factor) : c + (255 - c) * factor);
  return `rgb(${f(r)},${f(g)},${f(b)})`;
}

export function pillRadius(shape: PillShape): string {
  switch (shape) {
    case "rounded":
      return "rounded-2xl";
    case "square":
      return "rounded-md";
    default:
      return "rounded-full";
  }
}

export function pillBackground(
  mode: "idle" | "active" | "error",
  opacityPct: number,
): string {
  const alpha = Math.min(1, Math.max(0.4, opacityPct / 100));
  const base =
    mode === "error" ? "#3d1116" : mode === "active" ? "#1a1a21" : "#131317";
  return rgba(base, alpha);
}

