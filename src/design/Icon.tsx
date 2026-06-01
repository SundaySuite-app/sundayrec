/**
 * SundayRec redesign — icon set.
 *
 * Ported from the Claude Design handoff bundle (`sr-shell.jsx`). A single
 * 24×24 stroke-line icon set, drawn from compact SVG path strings. Every glyph
 * is split on `M` sub-paths so a single `<path>` string can encode several
 * strokes. This is the visual source of truth for the new shell + screens.
 */
import type { CSSProperties } from "react";

export const SR_ICONS = {
  home: "M3 10.5L12 3l9 7.5M5 9.5V20h5v-6h4v6h5V9.5",
  calendar:
    "M3 8.5h18M7 3v3m10-3v3M5 5.5h14a1 1 0 011 1V20a1 1 0 01-1 1H5a1 1 0 01-1-1V6.5a1 1 0 011-1z",
  live: "M12 12m-2 0a2 2 0 104 0a2 2 0 10-4 0M7.5 7.5a6 6 0 000 9M16.5 7.5a6 6 0 010 9M4.8 4.8a10 10 0 000 14.4M19.2 4.8a10 10 0 010 14.4",
  edit: "M4 7h10M4 12h16M4 17h7M17 14l3 0M18.5 12.5v3",
  search: "M11 11m-7 0a7 7 0 1014 0a7 7 0 10-14 0M20 20l-4-4",
  gear: "M12 9a3 3 0 100 6 3 3 0 000-6zM19.4 13a7.8 7.8 0 000-2l2-1.6-2-3.4-2.4 1a7.6 7.6 0 00-1.7-1l-.4-2.5h-4l-.4 2.5a7.6 7.6 0 00-1.7 1l-2.4-1-2 3.4L4.6 11a7.8 7.8 0 000 2l-2 1.6 2 3.4 2.4-1a7.6 7.6 0 001.7 1l.4 2.5h4l.4-2.5a7.6 7.6 0 001.7-1l2.4 1 2-3.4-2-1.6z",
  check: "M5 12.5l4.5 4.5L19 7",
  chevR: "M9 5l7 7-7 7",
  chevD: "M5 9l7 7 7-7",
  refresh: "M20 11a8 8 0 10-1.8 6M20 5v6h-6",
  mic: "M12 3a3 3 0 00-3 3v6a3 3 0 006 0V6a3 3 0 00-3-3zM5 11a7 7 0 0014 0M12 18v3",
  camera:
    "M3 8.5a2 2 0 012-2h2l1.5-2h7L18 6.5h1a2 2 0 012 2V18a2 2 0 01-2 2H5a2 2 0 01-2-2V8.5zM12 11a3.5 3.5 0 100 7 3.5 3.5 0 000-7z",
  video:
    "M3 7a2 2 0 012-2h9a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2V7zM16 10l5-3v10l-5-3",
  folder:
    "M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2V7z",
  file: "M6 3h8l5 5v12a1 1 0 01-1 1H6a1 1 0 01-1-1V4a1 1 0 011-1zM14 3v5h5",
  wave: "M3 12h2l2-6 3 14 3-18 3 14 2-4h3",
  plus: "M12 5v14M5 12h14",
  x: "M6 6l12 12M18 6L6 18",
  play: "M7 5l12 7-12 7V5z",
  skip: "M6 5l9 7-9 7V5zM18 5v14",
  loop: "M17 3l3 3-3 3M20 6H8a4 4 0 00-4 4M7 21l-3-3 3-3M4 18h12a4 4 0 004-4",
  normalize: "M4 20V10M9 20V4M14 20v-7M19 20V8M2 20h20",
  image:
    "M3 5a2 2 0 012-2h14a2 2 0 012 2v14a2 2 0 01-2 2H5a2 2 0 01-2-2V5zM8 11a2 2 0 100-4 2 2 0 000 4zM21 16l-5-5L5 21",
  drive: "M8 3h8l5 9H13L8 3zM3 12l4.5 7.5L12 12M16 12h5l-4.5 7.5H7.5",
  bell: "M6 9a6 6 0 1112 0c0 5 2 6 2 6H4s2-1 2-6zM9.5 20a2.5 2.5 0 005 0",
  mail: "M3 6a1 1 0 011-1h16a1 1 0 011 1v12a1 1 0 01-1 1H4a1 1 0 01-1-1V6zM3.5 6l8.5 7 8.5-7",
  webhook:
    "M12 7a3 3 0 10-2 5.3M9 17a3 3 0 105.8-1M15 17a3 3 0 10-2.6-4.6M9.5 14.5L12 10M14.5 14.5H9.5",
  globe:
    "M12 3a9 9 0 100 18 9 9 0 000-18zM3 12h18M12 3c2.5 2.5 3.5 6 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-6-3.5-9s1-6.5 3.5-9z",
  church: "M12 2l2 3v3l4 2v3M12 2L10 5v3L6 10v3M5 13h14v8H5v-8zM10 21v-4h4v4",
  update: "M21 12a9 9 0 11-3-6.7M21 4v4h-4",
  link: "M9 15l6-6M10.5 7.5l1.5-1.5a3.5 3.5 0 015 5l-2 2M13.5 16.5L12 18a3.5 3.5 0 01-5-5l2-2",
  sparkle: "M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8L12 3z",
  list: "M8 6h13M8 12h13M8 18h13M3.5 6h.01M3.5 12h.01M3.5 18h.01",
  clock: "M12 12m-9 0a9 9 0 1018 0a9 9 0 10-18 0M12 7v5l3.5 2",
  disk: "M5 4h11l4 4v11a1 1 0 01-1 1H5a1 1 0 01-1-1V5a1 1 0 011-1zM8 4v5h7V4M8 19v-5h8v5",
  info: "M12 12m-9 0a9 9 0 1018 0a9 9 0 10-18 0M12 8h.01M11 12h1v4h1",
  warn: "M12 4l9 16H3l9-16zM12 10v4M12 17h.01",
  zoomIn: "M11 11m-7 0a7 7 0 1014 0a7 7 0 10-14 0M20 20l-4-4M11 8v6M8 11h6",
  zoomOut: "M11 11m-7 0a7 7 0 1014 0a7 7 0 10-14 0M20 20l-4-4M8 11h6",
  download: "M12 3v12M7 10l5 5 5-5M5 21h14",
  scissors: "M6 6a2.5 2.5 0 103 4l11 8M6 18a2.5 2.5 0 113-4M9 10l11-7",
  shield: "M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6l7-3z",
  speaker: "M4 9v6h4l5 4V5L8 9H4zM16 9a3 3 0 010 6M18.5 7a6 6 0 010 10",
  eq: "M6 4v16M6 8h-.01M12 4v16M12 14h.01M18 4v16M18 10h.01",
  power: "M12 4v8M7 7a7 7 0 1010 0",
  upload: "M12 16V4M7 9l5-5 5 5M5 20h14",
} as const;

export type IconName = keyof typeof SR_ICONS;

export interface IconProps {
  name: IconName;
  size?: number;
  fill?: boolean;
  style?: CSSProperties;
  strokeWidth?: number;
}

export function Icon({
  name,
  size = 20,
  fill = false,
  style,
  strokeWidth = 1.7,
}: IconProps) {
  const d = SR_ICONS[name] || "";
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      style={style}
      fill={fill ? "currentColor" : "none"}
      stroke={fill ? "none" : "currentColor"}
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {d
        .split("M")
        .filter(Boolean)
        .map((seg, i) => (
          <path key={i} d={"M" + seg} />
        ))}
    </svg>
  );
}
