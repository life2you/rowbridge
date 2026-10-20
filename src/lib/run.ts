import type { SyncEvent, TablePreflight } from "./types";
import { formatInt } from "./format";

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

export interface LogEntry {
  t: number;
  level: string;
  table: string | null;
  msg: string;
}

export interface RunState {
  running: boolean;
  startedAt: number;
  totalRows: number;
  progress: Map<string, TableProgress>;
  logs: LogEntry[];
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
