import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import Dictionary from "./Dictionary";

const api = vi.hoisted(() => ({
  listDictionary: vi.fn().mockRejectedValue(new Error("database is offline")),
  listVocabSuggestions: vi.fn().mockResolvedValue([]),
  addDictionaryTerm: vi.fn(),
  acceptVocabSuggestion: vi.fn(),
  dismissVocabSuggestion: vi.fn(),
  setDictionaryStarred: vi.fn(),
  deleteDictionaryTerm: vi.fn(),
}));

vi.mock("../../lib/ipc", () => ({ api }));

describe("Dictionary", () => {
  it("shows a visible load error", async () => {
    render(<Dictionary />);

    await waitFor(() =>
      expect(screen.getByText("database is offline")).toBeInTheDocument(),
    );
  });
});
