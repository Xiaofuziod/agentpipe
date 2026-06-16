import { useState } from "react";
import { Composer } from "./composer/Composer";
import { Console } from "./console/Console";

export default function App() {
  const [view, setView] = useState<"composer" | "console">("composer");
  const [runPath, setRunPath] = useState<string | null>(null);
  return (
    <div style={{ fontFamily: "system-ui" }}>
      <nav style={{ display: "flex", gap: 8, padding: 8, borderBottom: "1px solid #eee" }}>
        <button onClick={() => setView("composer")} disabled={view === "composer"}>
          编排
        </button>
        <button onClick={() => setView("console")} disabled={view === "console"}>
          控制台
        </button>
      </nav>
      {view === "composer" ? (
        <Composer
          onRun={(p) => {
            setRunPath(p);
            setView("console");
          }}
        />
      ) : (
        <Console runPath={runPath} />
      )}
    </div>
  );
}
