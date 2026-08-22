import type { ReactNode } from "react";

export type Page = "home" | "dictionary" | "snippets" | "style" | "settings";

const NAV: { id: Page; label: string; icon: ReactNode }[] = [
  {
    id: "home",
    label: "Home",
    icon: (
      <path d="M3 10.5 12 3l9 7.5M5.5 9.5V20h13V9.5" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    ),
  },
  {
    id: "dictionary",
    label: "Dictionary",
    icon: (
      <path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15.5H6.5A2.5 2.5 0 0 0 4 21V5.5ZM8 7h8" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    ),
  },
  {
    id: "snippets",
    label: "Snippets",
    icon: (
      <path d="m8 8-4 4 4 4m8-8 4 4-4 4m-2-11-4 14" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    ),
  },
  {
    id: "style",
    label: "Style",
    icon: (
      <path d="M12 3a9 9 0 1 0 0 18h1.5a2.5 2.5 0 0 0 0-5H13a2 2 0 0 1 0-4h5a3 3 0 0 0 3-3c0-3-4-6-9-6Zm-4.5 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Zm4-3a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    ),
  },
  {
    id: "settings",
    label: "Settings",
    icon: (
      <path d="M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm7.4-3a7.4 7.4 0 0 0-.1-1.2l2-1.6-2-3.4-2.4 1a7.4 7.4 0 0 0-2-1.2L14.5 2h-4l-.4 2.6a7.4 7.4 0 0 0-2 1.2l-2.4-1-2 3.4 2 1.6a7.4 7.4 0 0 0 0 2.4l-2 1.6 2 3.4 2.4-1a7.4 7.4 0 0 0 2 1.2l.4 2.6h4l.4-2.6a7.4 7.4 0 0 0 2-1.2l2.4 1 2-3.4-2-1.6c.07-.4.1-.8.1-1.2Z" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    ),
  },
];

export default function Sidebar({
  page,
  onNavigate,
}: {
  page: Page;
  onNavigate: (p: Page) => void;
}) {
  return (
    <nav className="flex w-52 shrink-0 flex-col border-r border-white/5 bg-white/[0.02] p-4">
      <div className="mb-8 flex items-center gap-2 px-2 pt-2">
        <div className="flex h-7 w-7 items-center justify-center rounded-lg bg-gradient-to-br from-indigo-400 to-violet-600 text-sm font-bold text-white">
          F
        </div>
        <span className="text-sm font-semibold tracking-tight">FlowClone</span>
      </div>
      <div className="flex flex-col gap-1">
        {NAV.map((item) => (
          <button
            key={item.id}
            onClick={() => onNavigate(item.id)}
            className={`flex items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition ${
              page === item.id
                ? "bg-white/[0.07] font-medium text-white"
                : "text-neutral-400 hover:bg-white/[0.04] hover:text-neutral-200"
            }`}
          >
            <svg
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              className="h-4 w-4"
            >
              {item.icon}
            </svg>
            {item.label}
          </button>
        ))}
      </div>
      <div className="mt-auto px-3 text-xs text-neutral-700">v0.1.0 · M1</div>
    </nav>
  );
}
