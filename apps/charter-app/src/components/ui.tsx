import type { ButtonHTMLAttributes, ReactNode } from "react";

// Base components for the Kintrinsic design system. Thin wrappers over the
// classes in theme.css so screens stay declarative and consistent.

export function Seal({ size = 32 }: { size?: number }) {
  // The Kintrinsic mark: the flame with a budding second flame off its
  // upper-right shoulder (kindred-internal/brand `kintrinsic-bud`), gold on
  // the ink ground — the shipped app-icon palette. Replaces the Charter "C".
  const flame =
    "M24 6c5 6.5 8 10.5 8 15.5a8 8 0 0 1-16 0C16 16.5 19 12.5 24 6Z M24 13c2.8 3.7 4.5 5.9 4.5 8.7a4.5 4.5 0 0 1-9 0c0-2.8 1.7-5.1 4.5-8.7Z";
  return (
    <span
      className="seal"
      style={{ width: size, height: size, background: "#241b12" }}
      aria-hidden="true"
    >
      <svg viewBox="0 0 48 48" width={size * 0.82} height={size * 0.82} aria-hidden="true">
        <g transform="translate(-17.698 -9.223) scale(1.7447)" fill="#e0a458">
          <g transform="translate(-2.5 1)">
            <path fillRule="evenodd" d={flame} />
          </g>
          <g transform="translate(31.5 14) rotate(22) scale(0.4) translate(-24 -17.75)">
            <path fillRule="evenodd" d={flame} />
          </g>
        </g>
      </svg>
    </span>
  );
}

export function Card({
  children,
  tinted,
  className = "",
  ...rest
}: {
  children: ReactNode;
  tinted?: boolean;
  className?: string;
} & React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={`card ${tinted ? "card-2" : ""} ${className}`} {...rest}>
      {children}
    </div>
  );
}

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

export function Button({
  variant = "primary",
  block,
  className = "",
  children,
  ...rest
}: {
  variant?: ButtonVariant;
  block?: boolean;
} & ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      className={`btn btn-${variant} ${block ? "btn-block" : ""} ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

type Tone = "ok" | "warn" | "blocked" | "neutral";

export function Pill({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`pill pill-${tone}`}>{children}</span>;
}

export function Badge({ count }: { count: number }) {
  if (count <= 0) return null;
  return <span className="badge">{count}</span>;
}

export function Avatar({ name, color, size = 40 }: { name: string; color: string; size?: number }) {
  const initial = name.trim().charAt(0).toUpperCase() || "?";
  return (
    <span
      className="avatar"
      style={{ background: color, width: size, height: size, fontSize: size * 0.4 }}
      aria-hidden="true"
    >
      {initial}
    </span>
  );
}

export function SectionLabel({ children }: { children: ReactNode }) {
  return <div className="section-label">{children}</div>;
}

export function EmptyState({
  emoji = "✶",
  title,
  children,
}: {
  emoji?: string;
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-emoji" aria-hidden="true">
        {emoji}
      </div>
      <p className="empty-title">{title}</p>
      {children && <p className="card-sub">{children}</p>}
    </div>
  );
}

export function Banner({
  tone = "info",
  children,
}: {
  tone?: "warn" | "info" | "ok";
  children: ReactNode;
}) {
  return <div className={`banner banner-${tone}`}>{children}</div>;
}

export function ListRow({
  title,
  sub,
  leading,
  trailing,
  onClick,
}: {
  title: ReactNode;
  sub?: ReactNode;
  leading?: ReactNode;
  trailing?: ReactNode;
  onClick?: () => void;
}) {
  const Tag = onClick ? "button" : "div";
  return (
    <Tag className="row" onClick={onClick} type={onClick ? "button" : undefined}>
      {leading}
      <span className="row-main">
        <span className="row-title">{title}</span>
        {sub && (
          <>
            <br />
            <span className="row-sub">{sub}</span>
          </>
        )}
      </span>
      {trailing ?? (onClick ? <span className="row-chevron">›</span> : null)}
    </Tag>
  );
}
