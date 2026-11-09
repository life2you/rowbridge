import type { CursorMode, TableRow } from "../lib/types";

export interface TableCursor {
  dst_table?: string | null;
  cursor_start?: CursorMode | null;
  start_id?: string | null;
  end_id?: string | null;
  truncate_target?: boolean | null;
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
    ov.truncate_target != null ||
    (ov.start_id !== undefined && ov.start_id !== null) ||
    (ov.end_id !== undefined && ov.end_id !== null)
  );
}

/**
 * 估算某个 id 区间内会有多少行。
 *
 * 优先按闭区间跨度 [startId, endId]（即 endId - startId + 1）计算。
 * 针对自增单列整型主键，区间跨度就是最准确的记录数上限，
 * 避免因历史已删除数据的稀疏比率（total / max_pk）导致近期连续数据被错误打折估小。
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

  // 起始与结束均明确给出（闭区间 [start, end]）
  if (start !== null && end !== null) {
    if (end < start) return 0;
    const span = end - start + 1;
    return total > 0 ? Math.min(span, total) : span;
  }

  // 只有 start 没有 end（从 start 到源表 MAX，闭区间）
  if (start !== null && end === null) {
    const maxPk = t.max_pk ?? null;
    if (maxPk && maxPk > 0) {
      if (start > maxPk) return 0;
      const span = maxPk - start + 1;
      return total > 0 ? Math.min(span, total) : span;
    }
    // 缺少终点且无源 MAX，无法判定终点
    return null;
  }

  // 只有 end 没有 start（从头到指定 end）
  if (start === null && end !== null) {
    if (end <= 0) return 0;
    const span = end;
    return total > 0 ? Math.min(span, total) : span;
  }

  return total;
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
