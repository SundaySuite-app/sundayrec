import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import "./index.css";
// Redesign design-system tokens + component styles (the `sr-*` classes the new
// shell and screens compose). Loaded after index.css so it can build on the
// shared Tailwind reset.
import "./design/tokens.css";
// Initialise i18next before the first render so every component can call
// `t()` synchronously.
import "@/i18n";
import App from "./App";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Local data via Tauri IPC — a short stale time is plenty.
      staleTime: 5_000,
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </React.StrictMode>,
);
