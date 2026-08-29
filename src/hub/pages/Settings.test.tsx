import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import Settings from "./Settings";

const api = vi.hoisted(() => ({
  getSetting: vi.fn().mockResolvedValue(null),
  setSetting: vi.fn((key: string) =>
    key === "language"
      ? Promise.reject(new Error("database is read-only"))
      : Promise.resolve(),
  ),
  getHotkey: vi.fn().mockResolvedValue(["Right Shift"]),
  hotkeyOptions: vi.fn().mockResolvedValue(["F1", "CapsLock", "Right Shift"]),
  hotkeyWatcherStatus: vi.fn().mockResolvedValue("ready"),
  listMics: vi.fn().mockResolvedValue(["System microphone"]),
  micDeviceStatus: vi.fn().mockResolvedValue({
    configured: null,
    active: "System microphone",
    usingFallback: false,
  }),
  checkMicPermission: vi.fn().mockResolvedValue(true),
  accessibilityStatus: vi.fn().mockResolvedValue(true),
  inputMonitoringStatus: vi.fn().mockResolvedValue(true),
  autostartStatus: vi.fn().mockResolvedValue(false),
  localModelStatus: vi.fn().mockResolvedValue([
    { id: "base", label: "Base", approxMb: 60, downloaded: true },
  ]),
  localParakeetStatus: vi.fn().mockResolvedValue({
    id: "parakeet-tdt-0.6b-v3",
    available: false,
    downloaded: false,
  }),
  localLlmStatus: vi.fn().mockResolvedValue([
    { id: "qwen3-4b", label: "Qwen3", approxMb: 2400, downloaded: true },
  ]),
  appVersion: vi.fn().mockResolvedValue("0.3.1"),
  checkForUpdate: vi.fn().mockResolvedValue(null),
  installUpdate: vi.fn().mockResolvedValue(false),
  hotkeyLastSeen: vi.fn().mockResolvedValue([]),
  setFlowbarPreset: vi.fn().mockResolvedValue(undefined),
  setHotkey: vi.fn().mockResolvedValue(["Right Shift"]),
  setMicDevice: vi.fn().mockResolvedValue(undefined),
  setAutostart: vi.fn().mockResolvedValue(undefined),
  setLocalModel: vi.fn().mockResolvedValue(undefined),
  setLocalLlm: vi.fn().mockResolvedValue(undefined),
  downloadLocalModel: vi.fn().mockResolvedValue(undefined),
  downloadLocalLlm: vi.fn().mockResolvedValue(undefined),
  downloadLocalParakeet: vi.fn().mockResolvedValue("/model"),
  openAccessibilitySettings: vi.fn().mockResolvedValue(undefined),
  openInputMonitoringSettings: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../../lib/ipc", () => ({ api }));
vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn().mockResolvedValue(undefined),
  listen: vi.fn().mockResolvedValue(() => {}),
}));

describe("Settings", () => {
  it("restores the saved language and shows the save error", async () => {
    render(<Settings onRerunSetup={vi.fn()} />);

    const language = await screen.findByLabelText("Language");
    expect(language).toHaveValue("auto");
    fireEvent.change(language, { target: { value: "en" } });

    await waitFor(() => {
      expect(language).toHaveValue("auto");
      expect(screen.getByText(/Language could not be saved/)).toBeInTheDocument();
    });
  });
});
