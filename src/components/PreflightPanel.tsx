import type { Check, PreflightReport, Severity, TablePreflight } from "../lib/types";
import { formatInt } from "../lib/format";
import { Badge, Empty } from "./ui";
import { cn } from "../lib/format";

const ICON: Record<Severity, string> = {
  error: "✕",
  warning: "!",
  info: "i",
  ok: "✓",
};

const COLOR: Record<Severity, string> = {
  error: "text-[var(--color-err)]",
  warning: "text-[var(--color-warn)]",
  info: "text-[var(--color-muted)]",
  ok: "text-[var(--color-ok)]",
};

function CheckRow({ c }: { c: Check }) {
  return (
    <div className="flex items-start gap-2 py-[3px]">
      <span className={cn("mt-[1px] w-3 shrink-0 text-center text-[11px] font-bold", COLOR[c.severity])}>
        {ICON[c.severity]}
      </span>
      <span
        className={cn(
          "text-[12.5px] leading-[1.5]",
          c.severity === "error"
            ? "text-[#ffb3ae]"
            : c.severity === "warning"
              ? "text-[#e3c07b]"
              : "text-[var(--color-text)]",
        )}
      >
        {c.message}
      </span>
    </div>
  );
}

function TableCard({ t }: { t: TablePreflight }) {
  const errors = t.checks.filter((c) => c.severity === "error").length;
  const warns = t.checks.filter((c) => c.severity === "warning").length;

  return (
    <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)]">
      <div className="flex flex-wrap items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
        <span className="text-[13px] font-semibold">{t.table}</span>
        <span className="text-[11px] text-[var(--color-muted)]">
          {t.source_schema} → {t.target_schema}
        </span>

        <span className="ml-auto flex items-center gap-1.5">
          {t.syncable ? (
            <Badge tone="ok">可同步</Badge>
          ) : (
            <Badge tone="err">不可同步</Badge>
          )}
          {t.needs_create_table && t.create_table && (
            <Badge tone="accent">将建表</Badge>
          )}
          {t.target_rows_exists && <Badge tone="muted">目标表已存在</Badge>}
          {t.primary_key && <Badge tone="muted">PK {t.primary_key}</Badge>}
          {errors > 0 && <Badge tone="err">{errors} 阻塞</Badge>}
          {warns > 0 && <Badge tone="warn">{warns} 警告</Badge>}
        </span>
      </div>

      <div className="grid grid-cols-3 gap-3 border-b border-[var(--color-border)] px-3 py-2 text-[11px] text-[var(--color-muted)]">
        <div>
          源行数(估算) <span className="text-[var(--color-text)]">{formatInt(t.source_rows_estimate)}</span>
        </div>
        <div>
          源 MAX(pk) <span className="mono text-[var(--color-text)]">{t.source_max_pk ?? "—"}</span>
        </div>
        <div>
          目标 MAX(pk){" "}
          <span className="mono text-[var(--color-text)]">{t.target_max_pk ?? "—"}</span>
        </div>
      </div>

      <div className="px-3 py-2">
        {t.checks.map((c, i) => (
          <CheckRow key={`${c.code}-${i}`} c={c} />
        ))}
      </div>

      {t.create_table && (
        <details className="border-t border-[var(--color-border)]">
          <summary className="cursor-pointer px-3 py-1.5 text-[12px] text-[var(--color-accent)] hover:underline">
            查看将执行的建表语句（{t.create_table.changes.length} 处改写）
          </summary>
          <div className="px-3 pb-3">
            {t.create_table.changes.length > 0 && (
              <ul className="mb-2 space-y-0.5">
                {t.create_table.changes.map((ch, i) => (
                  <li key={i} className="text-[11.5px] text-[var(--color-muted)]">
                    · {ch.detail}
                  </li>
                ))}
              </ul>
            )}
            <pre className="max-h-64 overflow-auto rounded border border-[var(--color-border)] bg-[#0c0e12] p-2.5 text-[11.5px] leading-relaxed text-[#a9b4c6]">
              {t.create_table.ddl}
            </pre>
          </div>
        </details>
      )}
    </div>
  );
}

export function PreflightPanel({ report }: { report: PreflightReport | null }) {
  if (!report) {
    return (
      <Empty>
        点「预检」检查主键、列兼容性、目标表状态，并预览将执行的建表语句。
      </Empty>
    );
  }

  const blocking = report.tables.filter((t) => !t.syncable).length;
  const creatable = report.tables.filter((t) => t.needs_create_table && t.create_table).length;

  return (
    <div className="h-full overflow-y-auto p-3">
      <div className="mb-2.5 flex flex-wrap items-center gap-2 text-[12px]">
        <Badge tone="accent">源 {report.source_version}</Badge>
        <Badge tone="accent">目标 {report.target_version}</Badge>
        {report.same_instance && <Badge tone="warn">同实例跨库</Badge>}
        <span className="text-[var(--color-muted)]">
          {report.tables.length} 张表 · {report.tables.length - blocking} 可同步
          {creatable > 0 && ` · ${creatable} 张将新建`}
          {blocking > 0 && ` · ${blocking} 张被阻塞`}
        </span>
      </div>

      {report.checks.map((c, i) => (
        <CheckRow key={i} c={c} />
      ))}

      <div className="mt-2 space-y-2.5">
        {report.tables.map((t) => (
          <TableCard key={`${t.source_schema}.${t.table}`} t={t} />
        ))}
      </div>
    </div>
  );
}
