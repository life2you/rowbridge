import type { ReactNode } from "react";
import { cn } from "../lib/format";

export function Button({
  children,
  onClick,
  variant = "default",
  disabled,
  className,
  title,
  testid,
}: {
  children: ReactNode;
  onClick?: () => void;
  variant?: "default" | "primary" | "ghost" | "danger";
  disabled?: boolean;
  className?: string;
  title?: string;
  testid?: string;
}) {
  const base =
    "inline-flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-[13px] font-medium transition-colors disabled:opacity-40 disabled:cursor-not-allowed select-none";
  const styles = {
    default:
      "bg-[var(--color-panel-2)] border border-[var(--color-border)] hover:border-[#3d4553] hover:bg-[#252a35]",
    primary:
      "bg-[var(--color-accent)] text-white hover:brightness-110 border border-transparent",
    ghost: "hover:bg-[#222733] text-[var(--color-muted)] hover:text-[var(--color-text)]",
    danger: "bg-[#3d1f22] text-[var(--color-err)] border border-[#5c2a2f] hover:bg-[#4a2529]",
  }[variant];
  return (
    <button
      type="button"
      data-testid={testid}
      title={title}
      disabled={disabled}
      onClick={onClick}
      className={cn(base, styles, className)}
    >
      {children}
    </button>
  );
}

export function Checkbox({
  checked,
  onChange,
  disabled,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label: ReactNode;
  hint?: string;
}) {
  return (
    <label
      className={cn(
        "flex items-start gap-2 select-none",
        disabled ? "opacity-50 cursor-not-allowed" : "cursor-pointer",
      )}
      title={hint}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-[3px] h-3.5 w-3.5 shrink-0 accent-[var(--color-accent)]"
      />
      <span className="text-[13px] leading-5">{label}</span>
    </label>
  );
}

export function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: ReactNode;
  hint?: string;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] uppercase tracking-wide text-[var(--color-muted)]">
        {label}
      </span>
      {children}
      {hint && <span className="text-[11px] text-[#5c6478]">{hint}</span>}
    </label>
  );
}

export function Badge({
  children,
  tone = "muted",
}: {
  children: ReactNode;
  tone?: "muted" | "ok" | "warn" | "err" | "accent";
}) {
  const tones = {
    muted: "bg-[#242935] text-[var(--color-muted)]",
    ok: "bg-[#16301c] text-[var(--color-ok)]",
    warn: "bg-[#33280e] text-[var(--color-warn)]",
    err: "bg-[#3a1a1d] text-[var(--color-err)]",
    accent: "bg-[#16233d] text-[var(--color-accent)]",
  }[tone];
  return (
    <span
      className={cn(
        "inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium",
        tones,
      )}
    >
      {children}
    </span>
  );
}

export function Spinner({ className }: { className?: string }) {
  return (
    <span
      className={cn(
        "inline-block h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent",
        className,
      )}
    />
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-full items-center justify-center px-6 text-center text-[13px] text-[var(--color-muted)]">
      {children}
    </div>
  );
}
