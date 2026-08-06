/// <reference types="vite/client" />

// Types for Vite's compile-time constants (`import.meta.env.DEV` & co). The
// renderer is a Vite app but had never referenced them, so `tsc` had no
// declaration for `import.meta.env`. E5.1's fixture seam needs exactly one:
// `DEV`, which Vite replaces with a literal at build time, letting the shipped
// bundle drop its in-Tauri fixture branch entirely.
