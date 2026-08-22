import { useState } from "react";
import Sidebar, { type Page } from "./components/Sidebar";
import Home from "./pages/Home";
import Dictionary from "./pages/Dictionary";
import Snippets from "./pages/Snippets";
import Style from "./pages/Style";
import Settings from "./pages/Settings";

export default function Hub() {
  const [page, setPage] = useState<Page>("home");

  return (
    <div className="flex h-screen overflow-hidden">
      <Sidebar page={page} onNavigate={setPage} />
      <main className="min-w-0 flex-1">
        {page === "home" && <Home />}
        {page === "dictionary" && <Dictionary />}
        {page === "snippets" && <Snippets />}
        {page === "style" && <Style />}
        {page === "settings" && <Settings />}
      </main>
    </div>
  );
}
