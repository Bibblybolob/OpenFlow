import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import Onboarding from "./Onboarding";

const api = vi.hoisted(() => ({
  getHotkey: vi.fn().mockResolvedValue(["Right Shift"]),
  getSetting: vi.fn((key: string) =>
    Promise.resolve(key === "hotkeyMode" ? "toggle" : key === "sttLocalModel" ? "base" : null),
  ),
  setFlowbarVisible: vi.fn().mockResolvedValue(undefined),
  accessibilityStatus: vi.fn().mockResolvedValue(true),
  inputMonitoringStatus: vi.fn().mockResolvedValue(true),
  checkMicPermission: vi.fn().mockResolvedValue(true),
  localModelStatus: vi.fn().mockResolvedValue([
    { id: "base", label: "Base", approxMb: 60, downloaded: true },
  ]),
  setLocalModel: vi.fn().mockResolvedValue(undefined),
  downloadLocalModel: vi.fn().mockResolvedValue(undefined),
  setSetting: vi.fn((key: string) =>
    key === "onboardingComplete"
      ? Promise.reject(new Error("settings are read-only"))
      : Promise.resolve(),
  ),
  openAccessibilitySettings: vi.fn().mockResolvedValue(undefined),
  openInputMonitoringSettings: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../../lib/ipc", () => ({ api }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

describe("Onboarding", () => {
  it("shows a completion error and stays open when saving fails", async () => {
    render(
      <Onboarding
        pipelineState="idle"
        lastTranscriptId={null}
        onComplete={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Get started" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "A few permissions" })).toBeInTheDocument(),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Continue" })).not.toBeDisabled(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Set up transcription" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Try it out" })).toBeInTheDocument(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Skip for now" }));
    await waitFor(() =>
      expect(screen.getByText("settings are read-only")).toBeInTheDocument(),
    );
  });
});
