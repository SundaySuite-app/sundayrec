/**
 * SundayRec redesign — shared atoms.
 *
 * Ported from the Claude Design handoff (`sr-shell.jsx` + screen files). These
 * are thin presentational wrappers over the `sr-*` classes defined in
 * `tokens.css`; they carry no IPC. Functionality (real toggles, device data,
 * live meters) is wired in a later pass — for now props are plain values so the
 * redesign matches the mockup pixel-for-pixel.
 */
import type { CSSProperties, ReactNode } from "react";

import { Icon, type IconName } from "./Icon";

/* ── Toggle switch ──────────────────────────────────────────────────────── */
export function Toggle({ on = false }: { on?: boolean }) {
  return <div className={"sr-toggle" + (on ? " on" : "")} />;
}

/* ── Badge / chip ───────────────────────────────────────────────────────── */
export type BadgeKind = "muted" | "ok" | "warn" | "err" | "gold";
export function Badge({
  kind = "muted",
  children,
  dot,
}: {
  kind?: BadgeKind;
  children: ReactNode;
  dot?: boolean;
}) {
  return (
    <span className={"sr-badge " + kind}>
      {dot && <span className="bdot" />}
      {children}
    </span>
  );
}

/* ── Button ─────────────────────────────────────────────────────────────── */
export type BtnVariant = "ghost" | "gold" | "danger" | "ghost danger";
export function Btn({
  variant,
  sm,
  block,
  icon,
  iconFill,
  loading,
  disabled,
  children,
  style,
  onClick,
  ariaLabel,
  title,
  type = "button",
}: {
  variant?: BtnVariant;
  sm?: boolean;
  block?: boolean;
  icon?: IconName;
  iconFill?: boolean;
  /** Show an inline spinner in place of the leading icon and disable the button. */
  loading?: boolean;
  disabled?: boolean;
  children?: ReactNode;
  style?: CSSProperties;
  onClick?: () => void;
  /** Required for icon-only buttons (no visible text label). */
  ariaLabel?: string;
  title?: string;
  type?: "button" | "submit";
}) {
  const cls = ["sr-btn", variant, sm && "sm", block && "block"]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      className={cls}
      style={style}
      onClick={onClick}
      type={type}
      disabled={disabled || loading}
      aria-label={ariaLabel}
      aria-busy={loading || undefined}
      title={title}
    >
      {loading ? (
        <span
          className="sr-spinner"
          style={{ width: sm ? 14 : 16, height: sm ? 14 : 16 }}
        />
      ) : (
        icon && <Icon name={icon} size={sm ? 14 : 16} fill={iconFill} />
      )}
      {children}
    </button>
  );
}

/* ── Spinner ────────────────────────────────────────────────────────────── */
export function Spinner({ size = 16 }: { size?: number }) {
  return <span className="sr-spinner" style={{ width: size, height: size }} />;
}

/* ── Skeleton block (loading placeholder) ───────────────────────────────── */
export function Skeleton({
  w,
  h = 12,
  style,
}: {
  w?: number | string;
  h?: number | string;
  style?: CSSProperties;
}) {
  return (
    <span
      className="sr-skel"
      style={{ display: "block", width: w ?? "100%", height: h, ...style }}
      aria-hidden
    />
  );
}

/* ── Empty state (friendly "nothing here yet" placeholder) ──────────────── */
export function EmptyState({
  icon,
  title,
  desc,
  action,
}: {
  icon: IconName;
  title: ReactNode;
  desc?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="sr-empty">
      <div className="sr-empty-ico">
        <Icon name={icon} size={24} />
      </div>
      <div className="sr-empty-title">{title}</div>
      {desc && <div className="sr-empty-desc">{desc}</div>}
      {action && <div style={{ marginTop: 10 }}>{action}</div>}
    </div>
  );
}

/* ── Segmented option card ──────────────────────────────────────────────── */
export function SegOpt({
  sel,
  title,
  sub,
  badge,
}: {
  sel?: boolean;
  title: ReactNode;
  sub?: ReactNode;
  badge?: ReactNode;
}) {
  return (
    <div className={"sr-seg-opt" + (sel ? " sel" : "")}>
      <div className="t">{title}</div>
      {badge && (
        <div style={{ margin: "6px 0" }}>
          <Badge kind="warn">{badge}</Badge>
        </div>
      )}
      {sub && <div className="s">{sub}</div>}
    </div>
  );
}

/* ── Audio meter (n of total segments lit) ──────────────────────────────── */
export function Meter({ on = 4, total = 14 }: { on?: number; total?: number }) {
  return (
    <div className="sr-meter">
      {Array.from({ length: total }).map((_, i) => {
        const cls =
          i < on ? (i > total - 3 ? "hot" : i > total - 6 ? "mid" : "on") : "";
        return <span key={i} className={"seg " + cls} />;
      })}
    </div>
  );
}

/* ── Setting row (label/desc left, control right) ───────────────────────── */
export function SettingRow({
  title,
  desc,
  control,
}: {
  title: ReactNode;
  desc?: ReactNode;
  control: ReactNode;
}) {
  return (
    <div className="sr-srow">
      <div className="sr-grow">
        <div className="sr-srow-t">{title}</div>
        {desc && <div className="sr-srow-d">{desc}</div>}
      </div>
      <div style={{ flex: "0 0 auto" }}>{control}</div>
    </div>
  );
}

/* ── Card ───────────────────────────────────────────────────────────────── */
export function Card({
  title,
  icon,
  desc,
  action,
  children,
  pad = true,
  cls = "",
  style,
  anchor,
}: {
  title?: ReactNode;
  icon?: IconName;
  desc?: ReactNode;
  action?: ReactNode;
  children?: ReactNode;
  pad?: boolean;
  cls?: string;
  style?: CSSProperties;
  /** Deep-link target id. When a Home card sends the user to a specific
   *  setting, the matching Card carries `data-sr-anchor` so the screen can
   *  scroll it to center and flash it (see SettingsScreen's flash effect). */
  anchor?: string;
}) {
  return (
    <section
      className={"sr-card " + (pad ? "pad " : "") + cls}
      style={style}
      data-sr-anchor={anchor}
    >
      {(title || action) && (
        <div className="sr-card-head">
          <div>
            <div className="sr-card-title">
              {icon && <Icon name={icon} size={17} />}
              {title}
            </div>
            {desc && (
              <div className="sr-card-desc" style={{ marginTop: 6 }}>
                {desc}
              </div>
            )}
          </div>
          {action}
        </div>
      )}
      {children}
    </section>
  );
}

/* ── Device / summary card (Home rail, settings device list) ────────────── */
export function DeviceCard({
  icon,
  k,
  v,
  meta,
  badge,
  progress,
  onEdit,
  editLabel = "Endre",
}: {
  icon: IconName;
  k: ReactNode;
  v: ReactNode;
  meta?: ReactNode;
  badge?: ReactNode;
  progress?: number;
  /** When provided, renders the edit button and calls this on click.
   *  Omit it for a purely informational card (no dead button). */
  onEdit?: () => void;
  /** Visible + accessible label for the edit button (defaults to "Endre"). */
  editLabel?: string;
}) {
  return (
    <div className="sr-device">
      <div className="sr-device-ico">
        <Icon name={icon} size={19} />
      </div>
      <div className="sr-device-body">
        <div className="sr-device-k">{k}</div>
        <div className="sr-device-v">{v}</div>
        {meta && <div className="sr-device-meta">{meta}</div>}
        {badge && <div style={{ marginTop: 6 }}>{badge}</div>}
        {progress != null && (
          <div
            style={{
              marginTop: 9,
              height: 5,
              borderRadius: 3,
              background: "var(--sr-ink-700)",
              overflow: "hidden",
            }}
          >
            <div
              style={{
                width: progress + "%",
                height: "100%",
                background: "var(--sr-green)",
              }}
            />
          </div>
        )}
      </div>
      {onEdit && (
        <button
          className="sr-btn ghost sm"
          style={{ flex: "0 0 auto" }}
          onClick={onEdit}
          type="button"
          aria-label={editLabel}
        >
          {editLabel}
        </button>
      )}
    </div>
  );
}

/* ── Settings-list device row (selectable) ──────────────────────────────── */
export function DeviceRow({
  icon,
  name,
  meta,
  sel,
  badge,
}: {
  icon: IconName;
  name: ReactNode;
  meta?: ReactNode;
  sel?: boolean;
  badge?: ReactNode;
}) {
  return (
    <div className={"sr-device" + (sel ? " sel" : "")}>
      <div className="sr-device-ico">
        <Icon name={icon} size={19} />
      </div>
      <div className="sr-device-body">
        <div className="sr-row" style={{ gap: 9 }}>
          <span className="sr-device-v" style={{ marginTop: 0 }}>
            {name}
          </span>
          {badge}
        </div>
        <div className="sr-device-meta">{meta}</div>
      </div>
      {sel && (
        <Icon
          name="check"
          size={18}
          strokeWidth={2.4}
          style={{ color: "var(--sr-gold)", flex: "0 0 auto" }}
        />
      )}
    </div>
  );
}

/* ── Ready chip (Home "Klar til opptak" pills) ──────────────────────────── */
export function ReadyChip({ ok, label }: { ok?: boolean; label: ReactNode }) {
  return (
    <div
      className="sr-row"
      style={{
        gap: 7,
        padding: "5px 10px",
        borderRadius: "var(--sr-r-pill)",
        background: "var(--sr-line-faint)",
        border: "1px solid var(--sr-line)",
      }}
    >
      <span
        style={{
          color: ok ? "var(--sr-green)" : "var(--sr-gold)",
          display: "flex",
        }}
      >
        <Icon name={ok ? "check" : "warn"} size={14} strokeWidth={2.2} />
      </span>
      <span
        style={{ fontSize: 12.5, fontWeight: 600, color: "var(--sr-text-2)" }}
      >
        {label}
      </span>
    </div>
  );
}

/* ── Collapsible card (editor sections) ─────────────────────────────────── */
export function Collapsible({
  icon,
  title,
  meta,
  open,
  children,
}: {
  icon: IconName;
  title: ReactNode;
  meta?: ReactNode;
  open?: boolean;
  children?: ReactNode;
}) {
  return (
    <div className="sr-card pad">
      <div className="sr-row">
        <Icon name={icon} size={17} style={{ color: "var(--sr-text-3)" }} />
        <span className="sr-grow" style={{ fontSize: 15, fontWeight: 600 }}>
          {title}
        </span>
        {meta}
        <Icon
          name={open ? "chevD" : "chevR"}
          size={17}
          style={{ color: "var(--sr-text-3)" }}
        />
      </div>
      {open && children && <div style={{ marginTop: 16 }}>{children}</div>}
    </div>
  );
}
