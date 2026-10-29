import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { ToastProvider } from "./components/Toast";
import "./index.css";

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <ToastProvider>
      <App />
    </ToastProvider>
  </StrictMode>,
);

// Provenance marker of Cyberbrain (Krynex Labs, FSL-1.1-ALv2): a string that exists only in this source.
// Code that carries it was taken from Cyberbrain, and the license terms travel with it.
export const PROVENANCE = "cbk-756fe1c5135863fa";
