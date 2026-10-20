import type { CursorMode, TableRow } from "../lib/types";

export interface TableCursor {
  cursor_start?: CursorMode | null;
  start_id?: string | null;
  end_id?: string | null;
}

/** 全局默认游标设置 */
export interface GlobalCursor {
  cursor_start: CursorMode;
  start_id: string | null;
  end_id: string | null;
}

/** 解析某张表实际生效的游标（表级优先，其次全局）。 */
export function resolveCursor(
  _t: TableRow | undefined,
  ov: TableCursor | undefined,
  g: GlobalCursor,
): { mode: CursorMode; startId: string | null; endId: string | null } {
  const mode = ov?.cursor_start ?? g.cursor_start;
  // 表级显式设了就用表级（含空串表示不限）
  const startId =
    ov?.start_id !== undefined && ov?.start_id !== null
      ? ov.start_id || null
      : g.start_id;
  const endId =
    ov?.end_id !== undefined && ov?.end_id !== null
      ? ov.end_id || null
      : g.end_id;
  return { mode, startId, endId };
}

export function hasOverride(ov: TableCursor | undefined): boolean {
  if (!ov) return false;
  return (
    ov.cursor_start != null ||
    (ov.start_id !== undefined && ov.start_id !== null) ||
    (ov.end_id !== undefined && ov.end_id !== null)
  );
}

/**
 * 估算某个 id 区间内会有多少行。
 *
 * 源表只知道总行数（information_schema 的估算值）和 MAX(pk)，
 * 这里按主键近似均匀分布做个粗估，界面上要标明是估算。
 */
export function estimateRows(
  t: TableRow | undefined,
  c: { mode: CursorMode; startId: string | null; endId: string | null },
): number | null {
  if (!t) return null;
  const total = t.rows_estimate || 0;
  if (c.mode !== "custom") return total;

  const start = c.startId ? Number(c.startId) : null;
  const end = c.endId ? Number(c.endId) : null;
  if (start === null && end === null) return total;

  // 没有可用的主键上界就无法按比例推算
  const maxPk = t.max_pk ?? null;
  if (!maxPk || maxPk <= 0) {
    // 退化成「不知道」，宁可显示不确定也不给错的数字
    if (start === null) return total;
    return null;
  }
  const lo = start ?? 0;
  const hi = end ?? maxPk;
  const span = Math.max(1, hi - lo);
  return Math.max(0, Math.round((span / maxPk) * total));
}

export function summaryText(ov: TableCursor | undefined): string | null {
  if (!ov) return null;
  if (ov.cursor_start === "resume") return "续传";
  if (ov.cursor_start === "full") return "全量";
  const s = ov.start_id;
  const e = ov.end_id;
  if (s && e) return `${s}→${e}`;
  if (s) return `id>${s}`;
  if (e) return `≤${e}`;
  if (ov.cursor_start === "custom") return "自定义";
  return null;
}

const MODE_LABEL: Record<CursorMode, string> = {
  full: "全量",
  resume: "续传",
  custom: "自定义",
};

export { MODE_LABEL };
