import { useEffect, useRef, useState } from "react";
import type { SyncEvent, TablePreflight } from "../lib/types";
import { formatClock, formatInt, cn } from "../lib/format";

export interface TableProgress {
  table: string;
  rows: number;
  cursor: string;
  rowsPerSec: number;
  batchSize: number;
  status: "pending" | "running" | "created" | "done" | "failed" | "cancelled";
  error: string | null;
  elapsedMs: number;
}

export interface RunState {
  running: boolean;
  startedAt: number;
  totalRows: number;
  progress: Map<string, TableProgress>;
  logs: { t: number; level: string; table: string | null; msg: string }[];
  finished: boolean;
  cancelled: boolean;
  summary: {
    totalRows: number;
    succeeded: number;
    failed: number;
    partial: number;
    elapsedMs: number;
  } | null;
}

export const emptyRun = (): RunState => ({
  running: false,
  startedAt: 0,
  totalRows: 0,
  progress: new Map(),
  logs: [],
  finished: false,
  cancelled: false,
  summary: null,
});

/** 纯函数式的事件归约，便于测试与回溯。 */
export function reduceRun(
  state: RunState,
  ev: SyncEvent,
  tables: TablePreflight[],
): RunState {
  const next: RunState = {
    ...state,
    progress: new Map(state.progress),
    logs: state.logs,
  };

  switch (ev.type) {
    case "task_started": {
      const m = new Map<string, TableProgress>();
      for (const t of tables) {
        m.set(t.table, {
          table: t.table,
          rows: 0,
          cursor: "",
          rowsPerSec: 0,
          batchSize: 0,
          status: "pending",
          error: null,
          elapsedMs: 0,
        });
      }
      next.progress = m;
      next.running = true;
      next.startedAt = Date.now();
      next.logs = [];
      next.finished = false;
      next.cancelled = false;
      next.summary = null;
      next.totalRows = 0;
      break;
    }
    case "table_started": {
      const p = next.progress.get(ev.table);
      if (p) next.progress.set(ev.table, { ...p, status: "running" });
      break;
    }
    case "table_created": {
      const p = next.progress.get(ev.table);
      if (p) next.progress.set(ev.table, { ...p, status: "created" });
      next.logs = [
        ...state.logs,
        { t: Date.now(), level: "info", table: ev.table, msg: "已在目标端建表" },
      ];
      break;
    }
    case "batch_done": {
      const p = next.progress.get(ev.table);
      next.progress.set(ev.table, {
        table: ev.table,
        rows: ev.total_rows,
        cursor: ev.cursor,
        rowsPerSec: ev.rows_per_sec,
        batchSize: ev.batch_size,
        status: p?.status === "created" ? "created" : "running",
        error: null,
        elapsedMs: ev.elapsed_ms,
      });
      next.totalRows = ev.total_rows;
      break;
    }
    case "table_done": {
      const p = next.progress.get(ev.table);
      next.progress.set(ev.table, {
        table: ev.table,
        rows: ev.rows,
        cursor: p?.cursor ?? "",
        rowsPerSec: p?.rowsPerSec ?? 0,
        batchSize: p?.batchSize ?? 0,
        status: "done",
        error: null,
        elapsedMs: ev.elapsed_ms,
      });
      next.logs = [
        ...state.logs,
        {
          t: Date.now(),
          level: "ok",
          table: ev.table,
          msg: `完成 ${formatInt(ev.rows)} 行`,
        },
      ];
      break;
    }
    case "table_failed": {
      const p = next.progress.get(ev.table);
      next.progress.set(ev.table, {
        table: ev.table,
        rows: p?.rows ?? 0,
        cursor: p?.cursor ?? "",
        rowsPerSec: 0,
        batchSize: 0,
        status: "failed",
        error: ev.error,
        elapsedMs: 0,
      });
      next.logs = [
        ...state.logs,
        { t: Date.now(), level: "error", table: ev.table, msg: ev.error },
      ];
      break;
    }
    case "warning": {
      next.logs = [
        ...state.logs,
        { t: Date.now(), level: "warn", table: ev.table, msg: ev.message },
      ];
      break;
    }
    case "log": {
      next.logs = [
        ...state.logs,
        { t: Date.now(), level: ev.level, table: ev.table, msg: ev.message },
      ];
      break;
    }
    case "task_done": {
      next.running = false;
      next.finished = true;
      next.summary = {
        totalRows: ev.total_rows,
        succeeded: ev.succeeded,
        failed: ev.failed,
        partial: 0,
        elapsedMs: ev.elapsed_ms,
      };
      break;
    }
    case "cancelled": {
      next.running = false;
      next.finished = true;
      next.cancelled = true;
      break;
    }
  }
  // 只保留最近的日志，避免长任务把内存吃满
  if (next.logs.length > 2000) {
    next.logs = next.logs.slice(-2000);
  }
  return next;
}

const STATUS_LABEL: Record<TableProgress["status"], string> = {
  pending: "等待中",
  running: "同步中",
  created: "已建表",
  done: "已完成",
  failed: "失败",
  cancelled: "已取消",
};

function ProgressBar({
  done,
  total,
  status,
}: {
  done: number;
  total: number;
  status: TableProgress["status"];
}) {
  const indeterminate = total <= 0;
  const pct = indeterminate ? 0 : Math.min(100, (done / total) * 100);
  const color =
    status === "failed"
      ? "var(--color-err)"
      : status === "done"
        ? "var(--color-ok)"
        : "var(--color-accent)";
  return (
    <div className="h-1.5 w-full overflow-hidden rounded-full bg-[#222733]">
      <div
        className={cn(
          "h-full rounded-full transition-[width] duration-200",
          indeterminate && status === "running" && "animate-pulse",
        )}
        style={{
          width: indeterminate ? "100%" : `${pct}%`,
          background: color,
          opacity: indeterminate ? 0.35 : 1,
        }}
      />
    </div>
  );
}

export function RunPanel({
  run,
  tables,
  onCancel,
}: {
  run: RunState;
  tables: TablePreflight[];
  onCancel: () => void;
}) {
  const logRef = useRef<HTMLDivElement>(null);
  const [logExpanded, setLogExpanded] = useState(true);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [run.logs.length]);

  const list = tables.map(
    (t) =>
      run.progress.get(t.table) ?? {
        table: t.table,
        rows: 0,
        cursor: "",
        rowsPerSec: 0,
        batchSize: 0,
        status: "pending" as const,
        error: null,
        elapsedMs: 0,
      },
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b border-[var(--color-border)] px-3 py-2">
        <span className="text-[12px] font-semibold">
          {run.finished
            ? run.cancelled
              ? "已取消"
              : "同步完成"
            : "正在同步"}
        </span>
        {run.summary && (
          <span className="text-[12px] text-[var(--color-muted)]">
            {formatInt(run.summary.totalRows)} 行 · 成功 {run.summary.succeeded} · 失败{" "}
            {run.summary.failed}
            {run.summary.partial > 0 && ` · 未完成 ${run.summary.partial}`} ·{" "}
            {(run.summary.elapsedMs / 1000).toFixed(1)}s
          </span>
        )}
        <span className="ml-auto flex items-center gap-2">
          {run.running && (
            <button
              type="button"
              onClick={onCancel}
              className="rounded-md border border-[#5c2a2f] bg-[#3d1f22] px-3 py-1 text-[12px] font-medium text-[var(--color-err)] hover:bg-[#4a2529]"
            >
              取消
            </button>
          )}
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="space-y-0">
          {list.map((p) => {
            const pf = tables.find((t) => t.table === p.table);
            const total = pf?.source_rows_estimate ?? 0;
            return (
              <div
                key={p.table}
                className="border-b border-[var(--color-border)] px-3 py-2"
              >
                <div className="mb-1 flex items-center gap-2">
                  <span className="text-[12.5px] font-medium">{p.table}</span>
                  <span
                    className={cn(
                      "text-[11px]",
                      p.status === "failed"
                        ? "text-[var(--color-err)]"
                        : p.status === "done"
                          ? "text-[var(--color-ok)]"
                          : "text-[var(--color-muted)]",
                    )}
                  >
                    {STATUS_LABEL[p.status]}
                  </span>
                  <span className="ml-auto mono text-[11px] text-[var(--color-muted)]">
                    {formatInt(p.rows)}
                    {total > 0 && ` / ${formatInt(total)}`}
                    {p.rowsPerSec > 0 && ` · ${Math.round(p.rowsPerSec)} 行/s`}
                    {p.cursor && ` · 游标 ${p.cursor}`}
                  </span>
                </div>
                <ProgressBar done={p.rows} total={total} status={p.status} />
                {p.error && (
                  <div className="mt-1 text-[11.5px] text-[#ffb3ae] break-words">
                    {p.error}
                  </div>
                )}
              </div>
            );
          })}
        </div>

      </div>

      <div className="flex h-[38%] min-h-[120px] shrink-0 flex-col border-t border-[var(--color-border)]">
        <div className="flex items-center gap-2 px-3 py-1">
          <span className="text-[11px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
            日志
          </span>
          <span className="text-[11px] text-[#5c6478]">{run.logs.length} 条</span>
          <button
            type="button"
            onClick={() => setLogExpanded((v) => !v)}
            className="ml-auto text-[11px] text-[var(--color-muted)] hover:text-[var(--color-text)]"
          >
            {logExpanded ? "收起" : "展开"}
          </button>
        </div>
        <div
          ref={logRef}
          className={cn(
            "min-h-0 flex-1 overflow-y-auto bg-[#0c0e12] px-3 pb-2 mono text-[11.5px] leading-relaxed",
            !logExpanded && "hidden",
          )}
        >
          {run.logs.length === 0 && (
            <div className="text-[#5c6478]">暂无日志</div>
          )}
          {run.logs.map((l, i) => (
            <div key={i} className="flex gap-2">
              <span className="shrink-0 text-[#5c6478]">{formatClock(l.t)}</span>
              {l.table && <span className="shrink-0 text-[var(--color-accent)]">{l.table}</span>}
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
      </div>
    </div>
  );
}
