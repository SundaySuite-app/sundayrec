/**
 * Icon — ÉN komponent for alle ikonene i appen («Én flate»-skissen).
 *
 * 24×24, `stroke="currentColor"`, rund ende og skjøt. Dekorative som standard
 * (`aria-hidden`): etiketten står i knappen, ikke i tegningen. Gi `label` når
 * ikonet står alene uten knapp rundt seg.
 *
 * `record` er unntaket som har en egen farge: den fylte prikken bruker
 * `--rec` fra tokens.css, så ingen hex-literal ligger i TSX.
 * `play` og `pause` er fylte former (ikke streker), som i den gamle editoren.
 */

import type { ComponentChildren } from "preact";

export type IconName =
  | "record"
  | "recordings"
  | "gear"
  | "calendar"
  | "calendar-check"
  | "power"
  | "moon"
  | "bell"
  | "folder"
  | "rewind"
  | "mute"
  | "timer"
  | "scissors"
  | "trash"
  | "camera"
  | "music"
  | "wand"
  | "globe"
  | "refresh"
  | "shield"
  | "info"
  | "plus"
  | "arrow-left"
  | "arrow-right"
  | "arrow-up"
  | "arrow-down"
  | "x"
  | "play"
  | "pause"
  | "download"
  | "restore"
  | "sliders"
  | "laptop"
  | "mic"
  | "check"
  | "chevron-up"
  | "eye"
  | "copy";

/** Former som males med fyll i stedet for strek. */
const FILLED: ReadonlySet<IconName> = new Set(["play", "pause"]);

const GLYPHS: Record<IconName, ComponentChildren> = {
  eye: (
    <>
      <path d="M1.5 12S5.5 5 12 5s10.5 7 10.5 7-4 7-10.5 7S1.5 12 1.5 12z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  copy: (
    <>
      <rect x="9" y="9" width="12" height="12" rx="2" />
      <path d="M5 15V5a2 2 0 0 1 2-2h10" />
    </>
  ),
  record: (
    <>
      <circle cx="12" cy="12" r="9" />
      <circle
        cx="12"
        cy="12"
        r="4.5"
        stroke="none"
        style={{ fill: "var(--rec)" }}
      />
    </>
  ),
  recordings: (
    <>
      <path d="M7 2.5h10M5 5.5h14" />
      <rect x="3" y="8.5" width="18" height="13" rx="2.5" />
      <path d="M7.5 14v2M10 12v6M12.5 13v4M15 11v8M17.5 13.5v3" />
    </>
  ),
  gear: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
    </>
  ),
  calendar: (
    <>
      <rect x="3" y="4" width="18" height="17" rx="2" />
      <path d="M3 9h18M8 2v4M16 2v4" />
    </>
  ),
  "calendar-check": (
    <>
      <rect x="3" y="4" width="18" height="17" rx="2" />
      <path d="M3 9h18M8 2v4M16 2v4M9 15l2 2 4-4" />
    </>
  ),
  power: <path d="M12 2v10M18.4 6.6a9 9 0 1 1-12.8 0" />,
  moon: <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />,
  bell: (
    <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.9 1.9 0 0 0 3.4 0" />
  ),
  folder: (
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  ),
  rewind: <path d="M11 19l-9-7 9-7zM22 19l-9-7 9-7z" />,
  mute: <path d="M11 5L6 9H2v6h4l5 4zM23 9l-6 6M17 9l6 6" />,
  timer: (
    <>
      <circle cx="12" cy="13" r="8" />
      <path d="M12 9v4l2 2M9 2h6" />
    </>
  ),
  scissors: (
    <>
      <circle cx="6" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <path d="M20 4L8.1 15.9M14.5 14.5L20 20M8.1 8.1L12 12" />
    </>
  ),
  trash: <path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6" />,
  camera: (
    <>
      <path d="M15 10l5-3v10l-5-3z" />
      <rect x="2" y="6" width="13" height="12" rx="2" />
    </>
  ),
  music: (
    <>
      <path d="M9 18V5l12-2v13" />
      <circle cx="6" cy="18" r="3" />
      <circle cx="18" cy="16" r="3" />
    </>
  ),
  wand: (
    <path d="M15 4V2M15 16v-2M8 9h2M20 9h2M17.8 11.8l1.4 1.4M17.8 6.2l1.4-1.4M3 21l9-9M12.2 6.2l-1.4-1.4" />
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </>
  ),
  refresh: (
    <path d="M21 12a9 9 0 0 1-15.5 6.3L3 16M3 12a9 9 0 0 1 15.5-6.3L21 8M21 3v5h-5M3 21v-5h5" />
  ),
  shield: <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />,
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 16v-4M12 8h.01" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  "arrow-left": <path d="M19 12H5M12 19l-7-7 7-7" />,
  "arrow-right": <path d="M5 12h14M12 5l7 7-7 7" />,
  "arrow-up": <path d="M12 19V5M5 12l7-7 7 7" />,
  "arrow-down": <path d="M12 5v14M19 12l-7 7-7-7" />,
  x: <path d="M18 6L6 18M6 6l12 12" />,
  play: <path d="M8 5v14l11-7z" />,
  pause: (
    <>
      <rect x="6" y="5" width="4" height="14" rx="1" />
      <rect x="14" y="5" width="4" height="14" rx="1" />
    </>
  ),
  download: <path d="M12 3v12M7 10l5 5 5-5M5 21h14" />,
  restore: <path d="M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5" />,
  sliders: (
    <path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" />
  ),
  laptop: (
    <>
      <rect x="4" y="5" width="16" height="11" rx="1" />
      <path d="M2 19h20" />
    </>
  ),
  mic: (
    <>
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
    </>
  ),
  check: <path d="M5 12l5 5 9-10" />,
  "chevron-up": <path d="M6 15l6-6 6 6" />,
};

export interface IconProps {
  name: IconName;
  /** Kantlengde i piksler. Utelatt = CSS bestemmer (`1em` av omgivelsene). */
  size?: number;
  /** Strekbredde. Standard 1,8 som i skissen. */
  strokeWidth?: number;
  /** Tilgjengelig navn når ikonet står alene. Utelatt = dekorativt. */
  label?: string;
  class?: string;
}

export function Icon({
  name,
  size,
  strokeWidth = 1.8,
  label,
  class: className,
}: IconProps) {
  const filled = FILLED.has(name);
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill={filled ? "currentColor" : "none"}
      stroke={filled ? "none" : "currentColor"}
      stroke-width={strokeWidth}
      stroke-linecap="round"
      stroke-linejoin="round"
      class={className}
      data-icon={name}
      aria-hidden={label ? undefined : "true"}
      aria-label={label}
      role={label ? "img" : undefined}
    >
      {GLYPHS[name]}
    </svg>
  );
}
