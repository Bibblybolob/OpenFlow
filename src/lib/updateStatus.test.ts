import { describe, expect, it } from "vitest";
import { updateInstallStatus } from "./updateStatus";

describe("update install status", () => {
  it("shows the restart message only after an install", () => {
    expect(updateInstallStatus(true)).toBe("Installed. Restarting…");
  });

  it("does not claim an update was installed when none remains", () => {
    expect(updateInstallStatus(false)).toBe("No update is available.");
  });
});
