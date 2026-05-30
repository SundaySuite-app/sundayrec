// Extends Vitest's `expect` with @testing-library/jest-dom matchers
// (toBeInTheDocument, toBeVisible, ...).
import "@testing-library/jest-dom/vitest";

// Auto-unmount React trees between tests. Vitest's `globals` are off in this
// config, so Testing Library can't register its own afterEach — do it here so
// every test starts with a clean DOM (no leaked components across tests).
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
});
