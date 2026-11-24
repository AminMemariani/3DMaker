import "@fontsource-variable/inter";
import "./styles.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

// Scripted native E2E driver: compiled in only with `vite build --mode e2e`.
if (__E2E__) {
  import("./e2e/driver").then((m) => m.run());
}
