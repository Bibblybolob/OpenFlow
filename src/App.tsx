import { useEffect, useState } from "react";
import Hub from "./hub/Hub";

function App() {
  const [route, setRoute] = useState(() => window.location.hash || "#/hub");

  useEffect(() => {
    const onHash = () => setRoute(window.location.hash || "#/hub");
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  if (route.startsWith("#/flowbar")) {
    return <div className="h-screen" data-tauri-drag-region />;
  }
  return <Hub />;
}

export default App;
