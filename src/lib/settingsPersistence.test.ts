import { describe, expect, it, vi } from "vitest";
import { saveWithRollback } from "./settingsPersistence";

describe("saveWithRollback", () => {
  it("restores the previous value and reports a failed save", async () => {
    const restore = vi.fn();
    const report = vi.fn();
    const failure = new Error("database is locked");

    const saved = await saveWithRollback(
      "new",
      "old",
      () => Promise.reject(failure),
      restore,
      report,
    );

    expect(saved).toBe(false);
    expect(restore).toHaveBeenCalledWith("old");
    expect(report).toHaveBeenCalledWith(failure);
  });

  it("keeps the new value when the save succeeds", async () => {
    const restore = vi.fn();
    const report = vi.fn();

    const saved = await saveWithRollback(
      true,
      false,
      () => Promise.resolve(),
      restore,
      report,
    );

    expect(saved).toBe(true);
    expect(restore).not.toHaveBeenCalled();
    expect(report).not.toHaveBeenCalled();
  });
});
