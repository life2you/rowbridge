import { useMemo, useState } from "react";
import type { CursorMode, TableRow } from "../lib/types";
import {
  type GlobalCursor,
  type TableCursor,
  estimateRows,
  hasOverride,
  resolveCursor,
  summaryText,
} from "../lib/cursor";
import { cn, formatInt } from "../lib/format";
import { Badge, Button, Spinner } from "./ui";

type SortKey = "name" | "rows";

/** 中间区：已选表的批次表 + 逐表游标编辑。 */
export function BatchView({
  rows,
  overrides,
  onChange,
  global,
  onGlobalChange,
  batchSize,
  maxBatchSize,
  adaptiveBatch,
  onBatchChange,
  onRemove,
  onPreflight,
  onStart,
  preflighting,
  canPreflight,
  canStart,
  running,
  hasReport,
}: {
  rows: TableRow[];
  overrides: Record<string, TableCursor>;
  onChange: (table: string, ov: TableCursor) => void;
  global: GlobalCursor;
  onGlobalChange: (g: GlobalCursor) => void;
  /** 初始批大小（自适应开启时以此为起点） */
  batchSize: number;
  /** 一批最多多少行。宽表会自动压到 65535/列数，可能低于这个值 */
  maxBatchSize: number;
  adaptiveBatch: boolean;
  onBatchChange: (batchSize: number, maxBatchSize: number, adaptive: boolean) => void;
  onRemove: (table: string) => void;
  onPreflight: () => void;
  onStart: () => void;
  preflighting: boolean;
  canPreflight: boolean;
  canStart: boolean;
  running: boolean;
  hasReport: boolean;
}) {
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [batchStart, setBatchStart] = useState("");
  const [batchEnd, setBatchEnd] = useState("");
  const [batchMode, setBatchMode] = useState<CursorMode | "">("");
  const [detail, setDetail] = useState<string | null>(null);
  const [sort, setSort] = useState<SortKey>("name");

  const sorted = useMemo(() => {
    const list = [...rows];
    if (sort === "rows") {
      list.sort((a, b) => (b.rows_estimate || 0) - (a.rows_estimate || 0));
    } else {
      list.sort((a, b) => a.name.localeCompare(b.name));
    }
    return list;
  }, [rows, sort]);

  const est = (t: TableRow) =>
    estimateRows(t, resolveCursor(t, overrides[t.name], global));
  const totalEst = rows.reduce((s, t) => s + (est(t) ?? 0), 0);
  const overrideCount = rows.filter((t) => hasOverride(overrides[t.name])).length;

  function applyBatch() {
    const targets = selected.size > 0 ? [...selected] : rows.map((r) => r.name);
    for (const name of targets) {
      const prev = overrides[name] ?? {};
      onChange(name, {
        ...prev,
        ...(batchMode ? { cursor_start: batchMode } : {}),
        ...(batchStart ? { start_id: batchStart } : {}),
        ...(batchEnd ? { end_id: batchEnd } : {}),
      });
    }
  }

  function clearBatch() {
    const targets = selected.size > 0 ? [...selected] : rows.map((r) => r.name);
    for (const name of targets) onChange(name, {});
  }

  const detailRow = detail ? rows.find((r) => r.name === detail) : null;

  if (rows.length === 0) {
    return (
      <div className="flex min-w-0 flex-1 flex-col items-center justify-center gap-2 bg-[var(--color-bg)] px-6 text-center">
        <div className="text-[13px] font-semibold">还没有选表</div>
        <div className="max-w-[320px] text-[12px] leading-relaxed text-[var(--color-muted)]">
          在左侧勾选要同步的表。每张表可以单独设置自己的同步区间。
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-w-0 flex-1 flex-col bg-[var(--color-bg)]">
      {/* 头部 */}
      <div className="flex shrink-0 items-center gap-2.5 border-b border-[var(--color-border)] bg-[var(--color-panel)] px-3 py-2">
        <h2 className="text-[12.5px] font-semibold">同步批次</h2>
        <span className="text-[11px] text-[var(--color-muted)]">
          {rows.length} 张 · 约 {formatInt(totalEst)} 行
        </span>
        {overrideCount > 0 && (
          <Badge tone="accent">{overrideCount} 张自定义游标</Badge>
        )}
        <div className="ml-auto flex items-center gap-2">
          <Button onClick={onPreflight} disabled={!canPreflight} testid="preflight">
            {preflighting && <Spinner />}
            {preflighting ? "预检中…" : "预检"}
          </Button>
          <Button
            variant="primary"
            onClick={onStart}
            disabled={!canStart}
            testid="start"
            title={!hasReport ? "请先预检" : undefined}
          >
            {running ? <Spinner /> : null}
            {running ? "同步中…" : "开始同步"}
          </Button>
        </div>
      </div>

      {/* 批量工具条：第一行批量游标，第二行批大小等执行参数 */}
      <div className="shrink-0 space-y-1 border-b border-[var(--color-border)] bg-[#14171d] px-3 py-1.5 text-[11px]">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="text-[var(--color-muted)]">批量设置</span>
        <select
          value={batchMode}
          onChange={(e) => setBatchMode(e.target.value as CursorMode | "")}
          className="!py-0.5 !text-[11px]"
        >
          <option value="">不改模式</option>
          <option value="full">全量</option>
          <option value="resume">续传</option>
          <option value="custom">自定义</option>
        </select>
        <input
          value={batchStart}
          onChange={(e) => setBatchStart(e.target.value)}
          placeholder="起始 id"
          spellCheck={false}
          className="w-[92px] !py-0.5 !text-[11px]"
        />
        <span className="text-[var(--color-muted)]">到</span>
        <input
          value={batchEnd}
          onChange={(e) => setBatchEnd(e.target.value)}
          placeholder="结束 id"
          spellCheck={false}
          className="w-[92px] !py-0.5 !text-[11px]"
        />
        <Button onClick={applyBatch} className="!py-0.5 !text-[11px]">
          应用到{selected.size > 0 ? `选中 ${selected.size}` : "全部"}
        </Button>
        <button
          type="button"
          onClick={clearBatch}
          className="text-[11px] text-[var(--color-muted)] hover:text-[var(--color-text)]"
        >
          清除
        </button>
        <span className="ml-auto text-[10.5px] text-[#5c6478]">
          全局默认游标：
          <select
            value={global.cursor_start}
            onChange={(e) =>
              onGlobalChange({
                ...global,
                cursor_start: e.target.value as CursorMode,
              })
            }
            className="ml-1 !py-0.5 !text-[10.5px]"
          >
            <option value="full">全量</option>
            <option value="resume">续传</option>
            <option value="custom">自定义</option>
          </select>
        </span>
      </div>

      {/* 第二行：执行参数 */}
      <div className="flex flex-wrap items-center gap-1.5 border-t border-[#1e222b] pt-1">
        <span className="text-[10.5px] text-[var(--color-muted)]">执行参数</span>
        <span className="flex items-center gap-1 text-[10.5px] text-[#5c6478]">
          <span>批大小</span>
          <input
            type="number"
            min={1}
            value={batchSize}
            disabled={running}
            onChange={(e) =>
              onBatchChange(Number(e.target.value) || 1, maxBatchSize, adaptiveBatch)
            }
            data-testid="batch-size"
            className="w-[58px] !py-0.5 !text-[11px]"
            title="每批读取的起始行数（自适应开启时从此值开始爬升）"
          />
          <span>最多</span>
          <input
            type="number"
            min={1}
            value={maxBatchSize}
            disabled={running}
            onChange={(e) =>
              onBatchChange(batchSize, Number(e.target.value) || 1, adaptiveBatch)
            }
            data-testid="max-batch-size"
            className="w-[58px] !py-0.5 !text-[11px]"
            title="一批最多多少行；宽表会自动压到 65535/列数"
          />
          <span className="text-[#5c6478]">行/批</span>
        </span>
        <label
          className="flex cursor-pointer items-center gap-1 text-[10.5px] text-[#5c6478]"
          title="开启后按每批耗时自动调整；关闭则始终用「批大小」"
        >
          <input
            type="checkbox"
            checked={adaptiveBatch}
            disabled={running}
            onChange={(e) =>
              onBatchChange(batchSize, maxBatchSize, e.target.checked)
            }
            className="h-3 w-3 accent-[var(--color-accent)]"
          />
          自适应
        </label>
        <span className="text-[9.5px] text-[#4a5265]">
          {adaptiveBatch
            ? "按耗时自动爬升，不会超过「最多」"
            : "固定用「批大小」，不自动调整"}
        </span>
      </div>
      </div>

      {/* 表格 */}
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full min-w-[660px] border-collapse text-[12px]">
          <thead className="sticky top-0 z-10 bg-[var(--color-panel)]">
            <tr className="border-b border-[var(--color-border)]">
              <th className="w-7 px-2 py-1.5">
                <input
                  type="checkbox"
                  checked={selected.size === rows.length && rows.length > 0}
                  onChange={(e) =>
                    setSelected(e.target.checked ? new Set(rows.map((r) => r.name)) : new Set())
                  }
                  className="h-3 w-3 accent-[var(--color-accent)]"
                />
              </th>
              <th className="px-2 py-1.5 whitespace-nowrap text-left text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                <button
                  type="button"
                  onClick={() => setSort(sort === "name" ? "rows" : "name")}
                  className="hover:text-[var(--color-text)]"
                >
                  表 {sort === "name" ? "↑" : ""}
                </button>
              </th>
              <th className="w-[70px] px-2 py-1.5 whitespace-nowrap text-left text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                主键
              </th>
              <th className="w-[86px] px-2 py-1.5 whitespace-nowrap text-right text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                <button
                  type="button"
                  onClick={() => setSort(sort === "rows" ? "name" : "rows")}
                  className="hover:text-[var(--color-text)]"
                >
                  行数(估) {sort === "rows" ? "↓" : ""}
                </button>
              </th>
              <th className="w-[74px] px-2 py-1.5 whitespace-nowrap text-right text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                源 MAX
              </th>
              <th className="w-[92px] px-2 py-1.5 whitespace-nowrap text-left text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                模式
              </th>
              <th className="w-[92px] px-2 py-1.5 whitespace-nowrap text-left text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                起始 id
              </th>
              <th className="w-[92px] px-2 py-1.5 whitespace-nowrap text-left text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                结束 id
              </th>
              <th className="w-[84px] px-2 py-1.5 whitespace-nowrap text-right text-[10px] font-medium uppercase tracking-wide text-[var(--color-muted)]">
                预计行数
              </th>
              <th className="w-[52px] px-2 py-1.5" />
            </tr>
          </thead>
          <tbody>
            {sorted.map((t) => {
              const ov = overrides[t.name] ?? {};
              const eff = resolveCursor(t, ov, global);
              const rowEst = est(t);
              const on = selected.has(t.name);
              const overridden = hasOverride(ov);
              const exceedsMax =
                t.max_pk != null &&
                eff.endId != null &&
                Number(eff.endId) < t.max_pk;
              return (
                <tr
                  key={t.name}
                  className={cn(
                    "border-b border-[#1e222b]",
                    on && "bg-[#141c2b]",
                    !on && "hover:bg-[#151922]",
                  )}
                >
                  <td className="px-2 py-1">
                    <input
                      type="checkbox"
                      checked={on}
                      onChange={() =>
                        setSelected((prev) => {
                          const n = new Set(prev);
                          n.has(t.name) ? n.delete(t.name) : n.add(t.name);
                          return n;
                        })
                      }
                      className="h-3 w-3 accent-[var(--color-accent)]"
                    />
                  </td>
                  <td className="px-2 py-1">
                    <button
                      type="button"
                      data-testid={`detail-${t.name}`}
                      onClick={() => setDetail(t.name)}
                      className="max-w-[180px] truncate font-medium hover:text-[var(--color-accent)]"
                      title="查看详情"
                    >
                      {t.name}
                    </button>
                    {overridden && (
                      <span
                        className="ml-1 text-[9px] text-[var(--color-accent)]"
                        title="有独立游标设置"
                      >
                        ●
                      </span>
                    )}
                  </td>
                  <td className="mono px-2 py-1 text-[10.5px] text-[var(--color-muted)]">
                    {t.primary_key ?? "—"}
                  </td>
                  <td className="mono px-2 py-1 text-right text-[10.5px]">
                    {formatInt(t.rows_estimate)}
                  </td>
                  <td className="mono px-2 py-1 text-right text-[10.5px] text-[var(--color-muted)]">
                    {t.max_pk != null ? formatInt(t.max_pk) : "—"}
                  </td>
                  <td className="px-2 py-1">
                    <select
                      data-testid={`cursor-mode-${t.name}`}
                      value={ov.cursor_start ?? ""}
                      onChange={(e) =>
                        onChange(t.name, {
                          ...ov,
                          cursor_start:
                            (e.target.value as CursorMode) || undefined,
                        })
                      }
                      className="w-full !py-[1px] !text-[10.5px]"
                    >
                      <option value="">
                        跟随全局（{global.cursor_start === "full" ? "全量" : global.cursor_start === "resume" ? "续传" : "自定义"}）
                      </option>
                      <option value="full">全量</option>
                      <option value="resume">续传</option>
                      <option value="custom">自定义</option>
                    </select>
                  </td>
                  <td className="px-2 py-1">
                    <input
                      data-testid={`cursor-start-${t.name}`}
                      value={ov.start_id ?? ""}
                      disabled={eff.mode !== "custom"}
                      placeholder={
                        eff.mode === "custom"
                          ? "不限"
                          : global.start_id ?? "—"
                      }
                      onChange={(e) =>
                        onChange(t.name, { ...ov, start_id: e.target.value || "" })
                      }
                      spellCheck={false}
                      className="w-full !py-[1px] !text-[10.5px]"
                    />
                  </td>
                  <td className="px-2 py-1">
                    <input
                      data-testid={`cursor-end-${t.name}`}
                      value={ov.end_id ?? ""}
                      disabled={eff.mode !== "custom"}
                      placeholder={global.end_id ?? "不限"}
                      onChange={(e) =>
                        onChange(t.name, { ...ov, end_id: e.target.value || "" })
                      }
                      spellCheck={false}
                      className="w-full !py-[1px] !text-[10.5px]"
                    />
                  </td>
                  <td
                    className={cn(
                      "mono px-2 py-1 text-right text-[10.5px]",
                      overridden ? "text-[var(--color-accent)]" : "text-[#5c6478]",
                    )}
                    title={rowEst === null ? "缺少主键上界，无法估算" : "按主键区间粗估"}
                  >
                    {rowEst === null ? "?" : formatInt(rowEst)}
                    {exceedsMax && (
                      <span className="ml-0.5 text-[var(--color-warn)]" title="结束 id 小于源 MAX，区间之后的行不会同步">
                        !
                      </span>
                    )}
                  </td>
                  <td className="px-2 py-1 text-right">
                    <button
                      type="button"
                      onClick={() => onRemove(t.name)}
                      className="text-[10.5px] text-[var(--color-muted)] hover:text-[var(--color-err)]"
                    >
                      移除
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      {/* 底部汇总 */}
      <div className="flex shrink-0 items-center gap-3 border-t border-[var(--color-border)] bg-[var(--color-panel)] px-3 py-1.5 text-[11px] text-[var(--color-muted)]">
        <span>
          预计同步 <b className="text-[var(--color-text)]">{formatInt(totalEst)}</b> 行
        </span>
        <span className="text-[#5c6478]">
          {overrideCount} 张自定义 · {rows.length - overrideCount} 张跟随全局
        </span>
        <span className="ml-auto text-[10px] text-[#5c6478]">
          行数是按主键区间粗估，以预检为准
        </span>
      </div>

      {detailRow && (
        <TableDetailPanel
          row={detailRow}
          ov={overrides[detailRow.name] ?? {}}
          global={global}
          onChange={(ov) => onChange(detailRow.name, ov)}
          onClose={() => setDetail(null)}
          siblings={rows.filter((r) => r.name !== detailRow.name)}
          onPick={setDetail}
          overrides={overrides}
        />
      )}
    </div>
  );
}

/** 单表详情浮层（方案 G）：解释充分、主动指出区间风险。 */
function TableDetailPanel({
  row,
  ov,
  global,
  onChange,
  onClose,
  siblings,
  onPick,
  overrides,
}: {
  row: TableRow;
  ov: TableCursor;
  global: GlobalCursor;
  onChange: (ov: TableCursor) => void;
  onClose: () => void;
  siblings: TableRow[];
  onPick: (name: string) => void;
  overrides: Record<string, TableCursor>;
}) {
  const eff = resolveCursor(row, ov, global);
  const est = estimateRows(row, eff);
  const endBelowMax =
    row.max_pk != null && eff.endId != null && Number(eff.endId) < row.max_pk;

  const modes: { key: CursorMode; title: string; desc: string }[] = [
    { key: "custom", title: "自定义区间", desc: "从指定 id 开始，到指定 id 结束" },
    { key: "full", title: "全量", desc: "从头同步整张表" },
    { key: "resume", title: "续传", desc: "从目标表 MAX(id) 之后继续" },
  ];

  return (
    <div className="absolute inset-0 z-30 flex flex-col bg-[var(--color-bg)]/97 backdrop-blur-sm">
      <div className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-panel)] px-3 py-2">
        <h2 className="text-[13px] font-semibold">{row.name}</h2>
        <Badge tone="muted">主键 {row.primary_key ?? "—"}</Badge>
        <span className="text-[11px] text-[var(--color-muted)]">
          {formatInt(row.rows_estimate)} 行 · 源 MAX {row.max_pk != null ? formatInt(row.max_pk) : "—"}
        </span>
        <button
          type="button"
          onClick={() => onChange({})}
          className="ml-auto rounded px-2 py-0.5 text-[11px] text-[var(--color-muted)] hover:bg-[#222733] hover:text-[var(--color-text)]"
        >
          恢复跟随全局
        </button>
        <Button onClick={onClose} className="!py-1">
          完成
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        <div className="mb-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
          游标模式
        </div>
        <div className="mb-3 grid grid-cols-3 gap-2">
          {modes.map((m) => (
            <button
              key={m.key}
              type="button"
              onClick={() =>
                onChange({
                  ...ov,
                  cursor_start: m.key,
                  // 切到自定义时补一个空值，让输入框可编辑
                  ...(m.key === "custom" && ov.start_id == null
                    ? { start_id: "" }
                    : {}),
                })
              }
            >
              <div
                className={cn(
                  "rounded-lg border p-2 text-left transition-colors",
                  eff.mode === m.key
                    ? "border-[var(--color-accent)] bg-[#141c2b]"
                    : "border-[var(--color-border)] hover:border-[#3d4553]",
                )}
              >
                <div className="text-[12px] font-medium">{m.title}</div>
                <div className="mt-0.5 text-[10px] leading-snug text-[#5c6478]">
                  {m.desc}
                </div>
              </div>
            </button>
          ))}
        </div>

        <div className="mb-2 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
          同步区间
        </div>
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <label className="w-[52px] shrink-0 text-[11px] text-[var(--color-muted)]">
              起始 id
            </label>
            <input
              value={ov.start_id ?? ""}
              disabled={eff.mode !== "custom"}
              onChange={(e) => onChange({ ...ov, start_id: e.target.value })}
              placeholder="不限（从头开始）"
              spellCheck={false}
              className="flex-1 !text-[11.5px]"
            />
            <span className="w-[110px] shrink-0 text-[10px] text-[#5c6478]">
              不含此行
            </span>
          </div>
          <div className="flex items-center gap-2">
            <label className="w-[52px] shrink-0 text-[11px] text-[var(--color-muted)]">
              结束 id
            </label>
            <input
              value={ov.end_id ?? ""}
              disabled={eff.mode !== "custom"}
              onChange={(e) => onChange({ ...ov, end_id: e.target.value })}
              placeholder="不限（同步到最后）"
              spellCheck={false}
              className="flex-1 !text-[11.5px]"
            />
            <span className="w-[110px] shrink-0 text-[10px] text-[#5c6478]">
              含此行
            </span>
          </div>
        </div>

        <div className="mt-3 rounded-lg border border-[#22303f] bg-[#101419] p-2.5">
          <div className="flex items-baseline gap-2.5">
            <div className="mono text-[22px] font-semibold leading-none text-[var(--color-accent)]">
              {est === null ? "?" : formatInt(est)}
            </div>
            <div>
              <div className="text-[10px] text-[var(--color-muted)]">预计同步行数</div>
              <div className="text-[10px] text-[#5c6478]">
                按主键区间粗估，实际以预检为准
              </div>
            </div>
          </div>
          {endBelowMax && (
            <div className="mt-1.5 flex gap-1.5 text-[10.5px] text-[var(--color-warn)]">
              <span>⚠</span>
              <span>
                结束 id {eff.endId} 小于源 MAX {formatInt(row.max_pk!) }，
                {eff.endId} 之后的行不会被同步
              </span>
            </div>
          )}
          {eff.mode === "custom" && !eff.startId && !eff.endId && (
            <div className="mt-1.5 text-[10.5px] text-[var(--color-warn)]">
              ⚠ 选了自定义但没填区间，等同全量
            </div>
          )}
        </div>

        {siblings.length > 0 && (
          <>
            <div className="mb-1.5 mt-3 text-[10.5px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
              其他已选表
            </div>
            <div className="overflow-hidden rounded-lg border border-[var(--color-border)]">
              {siblings.map((s) => {
                const sOv = overrides[s.name] ?? {};
                const txt = summaryText(sOv);
                return (
                  <button
                    key={s.name}
                    type="button"
                    onClick={() => onPick(s.name)}
                    className="flex w-full items-center gap-2 border-b border-[#1e222b] px-2.5 py-1.5 text-left text-[11.5px] last:border-b-0 hover:bg-[#1a1e27]"
                  >
                    <span className="truncate">{s.name}</span>
                    <span
                      className={cn(
                        "text-[10px]",
                        txt ? "text-[var(--color-accent)]" : "text-[#5c6478]",
                      )}
                    >
                      {txt ?? "跟随全局"}
                    </span>
                    <span className="mono ml-auto text-[10px] text-[var(--color-muted)]">
                      {formatInt(estimateRows(s, resolveCursor(s, sOv, global)) ?? 0)} 行
                    </span>
                  </button>
                );
              })}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
