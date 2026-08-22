import { useEffect, useState } from "react";
import Hub from "./hub/Hub";
import FlowBar from "./flowbar/FlowBar";

function App() {
  const [route, setRoute] = useState(() => window.location.hash || "#/hub");

  useEffect(() => {
    const onHash = () => setRoute(window.location.hash || "#/hub");
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  if (route.startsWith("#/flowbar")) {
    return <FlowBar />;
  }
  return <Hub />;
}

export default App;
