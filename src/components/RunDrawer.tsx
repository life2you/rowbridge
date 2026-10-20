import { useEffect, useRef, useState } from "react";
import type { PreflightReport, TablePreflight } from "../lib/types";
import type { RunState } from "../lib/run";
import { cn, formatClock, formatDuration, formatInt } from "../lib/format";
import { Badge, Button, Spinner } from "./ui";

type Tab = "run" | "preflight" | "ddl" | "log";

const STATUS_TEXT: Record<string, string> = {
  pending: "等待",
  running: "同步中",
  created: "已建表",
  done: "完成",
  failed: "失败",
  cancelled: "已取消",
};

function statusColor(s: string) {
  if (s === "failed") return "var(--color-err)";
  if (s === "done") return "var(--color-ok)";
  if (s === "running" || s === "created") return "var(--color-accent)";
  return "var(--color-muted)";
}

/** 底部抽屉：运行 / 预检 / 建表 DDL / 日志，四个视角同处一栏。 */
export function RunDrawer({
  open,
  onToggle,
  run,
  report,
  preflighting,
  preflightError,
  startError,
  onCancel,
  onPreflight,
  canPreflight,
}: {
  open: boolean;
  onToggle: () => void;
  run: RunState;
  report: PreflightReport | null;
  preflighting: boolean;
  preflightError: string | null;
  startError: string | null;
  onCancel: () => void;
  onPreflight: () => void;
  canPreflight: boolean;
}) {
  const [tab, setTab] = useState<Tab>("run");
  const logRef = useRef<HTMLDivElement>(null);

  // 开跑自动切到运行页；有预检结果且没跑过就切到预检页
  useEffect(() => {
    if (run.running) setTab("run");
    else if (report && !run.finished) setTab("preflight");
  }, [run.running, run.finished, report]);

  useEffect(() => {
    if (tab === "log" && logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight;
    }
  }, [run.logs.length, tab]);

  const blocking = report?.tables.filter((t) => !t.syncable).length ?? 0;
  const runnable = report ? report.tables.length - blocking : 0;
  const withDdl = report?.tables.filter((t) => t.create_table) ?? [];

  const statusText = run.running
    ? "同步中"
    : run.finished
      ? run.cancelled
        ? "已取消"
        : "已完成"
      : report
        ? "预检完成"
        : "待预检";

  return (
    <div className="shrink-0 border-t border-[var(--color-border)] bg-[var(--color-panel)]">
      {/* 抽屉头 */}
      <div className="flex items-center gap-3 px-3 py-1.5">
        <button
          type="button"
          onClick={onToggle}
          className="flex items-center gap-1.5 text-[11.5px] font-semibold hover:text-[var(--color-accent)]"
        >
          <span className="text-[9px] text-[var(--color-muted)]">
            {open ? "▾" : "▸"}
          </span>
          运行
        </button>

        <span
          className="text-[11px]"
          style={{ color: run.running ? "var(--color-accent)" : "var(--color-muted)" }}
        >
          {statusText}
        </span>

        {open && (
          <div className="flex items-center gap-1">
            <TabBtn on={tab === "run"} onClick={() => setTab("run")}>
              进度{report ? ` ${report.tables.length}` : ""}
            </TabBtn>
            <TabBtn on={tab === "preflight"} onClick={() => setTab("preflight")}>
              预检{report ? ` ${runnable}` : ""}
              {blocking > 0 && (
                <span className="ml-1 text-[var(--color-err)]">{blocking}</span>
              )}
            </TabBtn>
            <TabBtn on={tab === "ddl"} onClick={() => setTab("ddl")}>
              建表 DDL{withDdl.length > 0 ? ` ${withDdl.length}` : ""}
            </TabBtn>
            <TabBtn on={tab === "log"} onClick={() => setTab("log")}>
              日志{run.logs.length > 0 ? ` ${run.logs.length}` : ""}
            </TabBtn>
          </div>
        )}

        {run.summary && (
          <span className="text-[11px] text-[var(--color-muted)]">
            {formatInt(run.summary.totalRows)} 行 · 成功 {run.summary.succeeded}
            {run.summary.partial > 0 && ` · 未完成 ${run.summary.partial}`}
            {run.summary.failed > 0 && ` · 失败 ${run.summary.failed}`} ·{" "}
            {formatDuration(run.summary.elapsedMs)}
          </span>
        )}

        <div className="ml-auto flex items-center gap-2">
          {!open && (
            <button
              type="button"
              onClick={onPreflight}
              disabled={!canPreflight}
              className="text-[11px] text-[var(--color-accent)] hover:underline disabled:opacity-40"
            >
              {preflighting ? "预检中…" : "预检"}
            </button>
          )}
          {run.running && (
            <Button
              onClick={onCancel}
              className="!border-[#5c2a2f] !bg-[#3d1f22] !text-[var(--color-err)] !py-1"
            >
              取消
            </Button>
          )}
        </div>
      </div>

      {/* 抽屉体 */}
      {open && (
        <div className="max-h-[300px] min-h-[150px] overflow-y-auto border-t border-[var(--color-border)]">
          {preflightError && (
            <div className="m-2 break-words rounded border border-[#5c2a2f] bg-[#2a1619] p-2 text-[11.5px] text-[#ffb3ae]">
              {preflightError}
            </div>
          )}
          {startError && (
            <div className="m-2 break-words rounded border border-[#5c2a2f] bg-[#2a1619] p-2 text-[11.5px] text-[#ffb3ae]">
              {startError}
            </div>
          )}

          {tab === "run" && <RunTab run={run} report={report} />}
          {tab === "preflight" && (
            <PreflightTab report={report} preflighting={preflighting} />
          )}
          {tab === "ddl" && <DdlTab tables={withDdl} />}
          {tab === "log" && (
            <div ref={logRef} className="p-2">
              <LogList logs={run.logs} />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function TabBtn({
  on,
  onClick,
  children,
}: {
  on: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "rounded px-2 py-0.5 text-[11px] transition-colors",
        on
          ? "bg-[#17233a] text-[var(--color-accent)]"
          : "text-[var(--color-muted)] hover:bg-[#1e222b] hover:text-[var(--color-text)]",
      )}
    >
      {children}
    </button>
  );
}

function RunTab({
  run,
  report,
}: {
  run: RunState;
  report: PreflightReport | null;
}) {
  const list = report?.tables ?? [];
  if (list.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-[11.5px] text-[var(--color-muted)]">
        还没有可运行的批次。先在两侧选好库，勾选表，再点「预检」。
      </div>
    );
  }

  return (
    <div>
      {list.map((t) => {
        const p = run.progress.get(t.table);
        const status = p?.status ?? "pending";
        const rows = p?.rows ?? 0;
        const total = t.source_rows_estimate || 0;
        const pct = total > 0 ? Math.min(100, (rows / total) * 100) : 0;
        return (
          <div
            key={t.table}
            className="flex items-center gap-2.5 border-b border-[#1e222b] px-3 py-[5px] text-[11.5px]"
          >
            <span className="mono w-[126px] shrink-0 truncate">{t.table}</span>
            <span className="h-1.5 min-w-[60px] flex-1 overflow-hidden rounded-full bg-[#222733]">
              <i
                className="block h-full transition-[width] duration-200"
                style={{
                  width:
                    status === "pending"
                      ? "0%"
                      : status === "done"
                        ? "100%"
                        : total > 0
                          ? `${pct}%`
                          : "100%",
                  background: statusColor(status),
                  opacity: status === "running" && total <= 0 ? 0.4 : 1,
                }}
              />
            </span>
            <span
              className="w-[42px] shrink-0 text-[11px]"
              style={{ color: statusColor(status) }}
            >
              {STATUS_TEXT[status] ?? status}
            </span>
            <span className="mono w-[188px] shrink-0 text-right text-[10.5px] text-[var(--color-muted)]">
              {formatInt(rows)}
              {total > 0 && ` / ${formatInt(total)}`}
              {p && p.rowsPerSec > 0 && ` · ${Math.round(p.rowsPerSec)}/s`}
              {p?.cursor && ` · 游标 ${p.cursor}`}
            </span>
          </div>
        );
      })}
      {run.logs.filter((l) => l.level === "error").map((l, i) => (
        <div key={i} className="px-3 py-1 text-[11px] text-[#ffb3ae]">
          {l.table ? `${l.table}: ` : ""}
          {l.msg}
        </div>
      ))}
    </div>
  );
}

function PreflightTab({
  report,
  preflighting,
}: {
  report: PreflightReport | null;
  preflighting: boolean;
}) {
  if (preflighting && !report) {
    return (
      <div className="flex items-center gap-2 px-3 py-6 text-[11.5px] text-[var(--color-muted)]">
        <Spinner /> 正在检查主键、列兼容性与目标表状态…
      </div>
    );
  }
  if (!report) {
    return (
      <div className="px-3 py-6 text-center text-[11.5px] text-[var(--color-muted)]">
        点「预检」检查主键是否合格、目标表是否需要新建、列是否兼容。
      </div>
    );
  }

  return (
    <div className="p-2">
      <div className="mb-1.5 flex flex-wrap items-center gap-1.5 px-1">
        <Badge tone="accent">源 {report.source_version}</Badge>
        <Badge tone="accent">目标 {report.target_version}</Badge>
        {report.same_instance && <Badge tone="warn">同实例跨库</Badge>}
      </div>

      {report.checks.map((c, i) => (
        <CheckLine key={i} c={c} />
      ))}

      <div className="mt-1.5 space-y-1">
        {report.tables.map((t) => (
          <div
            key={t.table}
            className="rounded border border-[var(--color-border)] bg-[#14171d] px-2 py-1.5"
          >
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="text-[12px] font-semibold">{t.table}</span>
              {t.syncable ? (
                <Badge tone="ok">可同步</Badge>
              ) : (
                <Badge tone="err">不可同步</Badge>
              )}
              {t.needs_create_table && t.create_table && (
                <Badge tone="accent">将建表</Badge>
              )}
              {t.target_rows_exists && <Badge tone="muted">目标已存在</Badge>}
              {t.primary_key && <Badge tone="muted">PK {t.primary_key}</Badge>}
              <span className="ml-auto mono text-[10px] text-[var(--color-muted)]">
                {formatInt(t.source_rows_estimate)} 行 · MAX(pk) {t.source_max_pk ?? "—"}
              </span>
            </div>
            {t.checks.map((c, i) => (
              <CheckLine key={i} c={c} small />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function CheckLine({
  c,
  small,
}: {
  c: PreflightReport["checks"][number];
  small?: boolean;
}) {
  const icon =
    c.severity === "error"
      ? "✕"
      : c.severity === "warning"
        ? "!"
        : c.severity === "info"
          ? "i"
          : "✓";
  const color =
    c.severity === "error"
      ? "var(--color-err)"
      : c.severity === "warning"
        ? "var(--color-warn)"
        : c.severity === "info"
          ? "var(--color-muted)"
          : "var(--color-ok)";
  return (
    <div
      className={cn(
        "flex items-start gap-1.5",
        small ? "py-[1px] pl-1" : "py-[2px]",
      )}
    >
      <span
        className="w-3 shrink-0 text-center text-[10px] font-bold"
        style={{ color }}
      >
        {icon}
      </span>
      <span
        className={cn(
          "leading-[1.5]",
          small ? "text-[11px]" : "text-[11.5px]",
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

function DdlTab({ tables }: { tables: TablePreflight[] }) {
  if (tables.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-[11.5px] text-[var(--color-muted)]">
        没有需要新建的表。目标表已存在时只补数据，不改结构。
      </div>
    );
  }
  return (
    <div className="space-y-2 p-2">
      {tables.map((t) => (
        <details key={t.table} open={tables.length <= 2}>
          <summary className="cursor-pointer text-[11.5px]">
            <span className="font-semibold">{t.table}</span>
            <span className="ml-2 text-[var(--color-muted)]">
              {t.create_table!.changes.length} 处改写
            </span>
          </summary>
          <div className="mt-1 pl-3">
            {t.create_table!.changes.map((ch, i) => (
              <div key={i} className="text-[10.5px] text-[var(--color-muted)]">
                · {ch.detail}
              </div>
            ))}
            <pre className="mt-1 max-h-40 overflow-auto rounded border border-[var(--color-border)] bg-[#0c0e12] p-2 text-[10.5px] leading-relaxed text-[#a9b4c6]">
              {t.create_table!.ddl}
            </pre>
          </div>
        </details>
      ))}
    </div>
  );
}

function LogList({ logs }: { logs: RunState["logs"] }) {
  if (logs.length === 0) {
    return (
      <div className="py-4 text-center text-[11px] text-[#5c6478]">暂无日志</div>
    );
  }
  return (
    <div className="mono text-[11px] leading-relaxed">
      {logs.map((l, i) => (
        <div key={i} className="flex gap-2">
          <span className="shrink-0 text-[#5c6478]">{formatClock(l.t)}</span>
          {l.table && (
            <span className="shrink-0 text-[var(--color-accent)]">{l.table}</span>
          )}
          <span
            className={cn(
              "break-all",
              l.level === "error"
                ? "text-[var(--color-err)]"
                : l.level === "warn"
                  ? "text-[var(--color-warn)]"
                  : l.level === "ok"
                    ? "text-[var(--color-ok)]"
                    : "text-[#a9b4c6]",
            )}
          >
            {l.msg}
          </span>
        </div>
      ))}
    </div>
  );
}