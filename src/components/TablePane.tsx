import { useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import type { SchemaInfo, TableRow } from "../lib/types";
import type { ConnectionDraft } from "./ConnectionEditor";
import { cn, formatInt } from "../lib/format";
import { Badge, Button, Spinner } from "./ui";

/**
 * 源 / 目标的一侧面板。
 *
 * 连接信息默认压成一行——它是一次性配置，不该常驻占地方。
 * 点「切换」才展开完整的连接表单。
 */
export function TablePane({
  side,
  conn,
  onConnChange,
  schemas,
  schema,
  onSchemaChange,
  tables,
  loading,
  selected,
  onToggle,
  onSelectAll,
  totalRows,
  disabled,
  willCreate = 0,
  existing = 0,
}: {
  side: "src" | "dst";
  conn: ConnectionDraft;
  onConnChange: (c: ConnectionDraft) => void;
  schemas: SchemaInfo[];
  schema: string | null;
  onSchemaChange: (s: string | null) => void;
  tables: TableRow[];
  loading?: boolean;
  selected: Set<string>;
  onToggle: (name: string) => void;
  onSelectAll: (names: string[]) => void;
  totalRows: number;
  disabled: boolean;
  willCreate?: number;
  existing?: number;
}) {
  const [showConn, setShowConn] = useState(false);
  const [filter, setFilter] = useState("");
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<
    { ok: true; version: string; schemas: number } | { ok: false; error: string } | null
  >(null);

  const isSrc = side === "src";
  const dot = isSrc ? "var(--color-accent)" : "var(--color-ok)";

  const set = <K extends keyof ConnectionDraft>(k: K, v: ConnectionDraft[K]) =>
    onConnChange({ ...conn, [k]: v });

  async function handleTest() {
    setTesting(true);
    setTestResult(null);
    try {
      const r = await api.testConnection(conn);
      setTestResult({ ok: true, version: r.version, schemas: r.schemas });
      if (conn.password) {
        await api.savePassword(conn.id, conn.password);
        onConnChange({ ...conn, passwordSaved: true, connection_id: conn.id });
      }
      // 连上后收起表单，把空间还给表清单
      setShowConn(false);
    } catch (e) {
      setTestResult({ ok: false, error: errorMessage(e) });
    } finally {
      setTesting(false);
    }
  }

  const visible = tables.filter((t) =>
    filter.trim() ? t.name.toLowerCase().includes(filter.trim().toLowerCase()) : true,
  );
  const syncable = visible.filter((t) => !t.unsupported_reason && !t.is_view);
  const allOn = syncable.length > 0 && syncable.every((t) => selected.has(t.name));

  return (
    <div
      className={cn(
        // 侧栏定宽：中间那张批次表需要足够的横向空间放 7 列，
        // 三栏等分的话表头会被挤成竖排。
        "flex w-[236px] shrink-0 flex-col",
        !isSrc && "bg-[#12151b]",
      )}
    >
      {/* 连接行 */}
      <div className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
        <i className="h-2 w-2 shrink-0 rounded-full" style={{ background: dot }} />
        <span className="text-[12.5px] font-semibold">{isSrc ? "源" : "目标"}</span>
        <span
          className="mono min-w-0 flex-1 truncate text-[10.5px] text-[var(--color-muted)]"
          title={`${conn.host}:${conn.port} · ${conn.username}`}
        >
          {conn.host}:{conn.port}
        </span>
        {testResult?.ok && (
          <Badge tone="ok">
            <span className="whitespace-nowrap">v{testResult.version}</span>
          </Badge>
        )}
        <button
          type="button"
          data-testid={`${side}-toggle-conn`}
          disabled={disabled}
          onClick={() => setShowConn((v) => !v)}
          className="shrink-0 rounded px-1.5 py-0.5 text-[11px] text-[var(--color-muted)] hover:bg-[#222733] hover:text-[var(--color-text)]"
        >
          {showConn ? "收起" : "切换"}
        </button>
      </div>

      {/* 连接表单（按需展开） */}
      {showConn && (
        <div className="shrink-0 space-y-2 border-b border-[var(--color-border)] bg-[var(--color-panel)] p-3">
          <div className="grid grid-cols-[1fr_74px] gap-2">
            <input
              data-testid={`${side}-host`}
              value={conn.host}
              onChange={(e) => set("host", e.target.value)}
              placeholder="127.0.0.1"
              spellCheck={false}
              className="!text-[12px]"
            />
            <input
              data-testid={`${side}-port`}
              type="number"
              value={conn.port}
              onChange={(e) => set("port", Number(e.target.value) || 3306)}
              className="!text-[12px]"
            />
          </div>
          <div className="grid grid-cols-2 gap-2">
            <input
              value={conn.username}
              onChange={(e) => set("username", e.target.value)}
              placeholder="用户名"
              spellCheck={false}
              className="!text-[12px]"
            />
            <input
              data-testid={`${side}-password`}
              type="password"
              value={conn.password ?? ""}
              onChange={(e) => set("password", e.target.value)}
              placeholder={conn.passwordSaved ? "已保存" : "密码"}
              className="!text-[12px]"
            />
          </div>
          <Button
            onClick={handleTest}
            disabled={testing}
            className="w-full"
            testid={`${side}-test`}
          >
            {testing && <Spinner />}
            {testing ? "连接中…" : "测试并连接"}
          </Button>
          {testResult && !testResult.ok && (
            <div className="break-words rounded border border-[#5c2a2f] bg-[#2a1619] px-2 py-1.5 text-[11.5px] text-[var(--color-err)]">
              {testResult.error}
            </div>
          )}
        </div>
      )}

      {/* 库选择 */}
      <div className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-panel-2)] px-3 py-2">
        <select
          data-testid={`${side}-schema`}
          value={schema ?? ""}
          onChange={(e) => onSchemaChange(e.target.value || null)}
          className="min-w-0 flex-1 !py-1 !text-[12.5px] !font-semibold"
          disabled={disabled}
        >
          <option value="">— 选择库 —</option>
          {schemas.map((s) => (
            <option key={s.name} value={s.name}>
              {s.name}
            </option>
          ))}
          {schema && !schemas.some((s) => s.name === schema) && (
            <option value={schema}>{schema}（将新建）</option>
          )}
        </select>
        {isSrc ? (
          <Badge tone={tables.length > 0 ? "ok" : "muted"}>{tables.length} 表</Badge>
        ) : (
          <Badge tone={willCreate + existing > 0 ? "warn" : "muted"}>
            {willCreate + existing === 0
              ? "未预检"
              : existing > 0
                ? `${existing} 已存在`
                : "空库"}
          </Badge>
        )}
      </div>

      {/* 表清单 */}
      {isSrc ? (
        <>
          <div className="shrink-0 px-3 pt-2">
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="搜索表名…"
              className="w-full !py-1 !text-[12px]"
              spellCheck={false}
            />
          </div>
          <div className="flex shrink-0 items-center justify-between px-3 py-1.5">
            <button
              type="button"
              disabled={disabled || syncable.length === 0}
              onClick={() => onSelectAll(allOn ? [] : syncable.map((t) => t.name))}
              className="text-[11px] text-[var(--color-accent)] hover:underline disabled:opacity-40"
            >
              {allOn ? "取消全选" : `全选可同步 (${syncable.length})`}
            </button>
            <span className="text-[11px] text-[var(--color-muted)]">
              已选 <b className="text-[var(--color-text)]">{selected.size}</b>
            </span>
          </div>

          <div className="min-h-0 flex-1 overflow-y-auto">
            {loading && (
              <div className="flex items-center gap-2 px-3 py-3 text-[11.5px] text-[var(--color-muted)]">
                <Spinner /> 读取表结构…
              </div>
            )}
            {!loading && !schema && (
              <div className="px-3 py-3 text-[11.5px] text-[var(--color-muted)]">
                先在上方选择一个库
              </div>
            )}
            {visible.map((t) => {
              const bad = !!t.unsupported_reason || t.is_view;
              return (
                <div
                  key={t.name}
                  title={t.unsupported_reason ?? undefined}
                  className={cn(
                    "flex items-center gap-2 px-3 py-[3.5px] text-[12px]",
                    bad && "opacity-40",
                    !bad && "hover:bg-[#1a1e27]",
                    selected.has(t.name) && !bad && "bg-[#17233a]",
                  )}
                >
                  <input
                    type="checkbox"
                    data-testid={`table-${t.name}`}
                    checked={selected.has(t.name)}
                    disabled={bad || disabled}
                    onChange={() => onToggle(t.name)}
                    className="h-3.5 w-3.5 shrink-0 accent-[var(--color-accent)]"
                  />
                  <span className="truncate">{t.name}</span>
                  <span className="ml-auto flex shrink-0 items-center gap-1.5 text-[10px]">
                    {bad ? (
                      <span className="text-[var(--color-err)]">
                        {t.is_view ? "视图" : "不支持"}
                      </span>
                    ) : (
                      <>
                        {t.primary_key && (
                          <span className="text-[var(--color-dim,#5c6478)]">
                            {t.primary_key}
                          </span>
                        )}
                        <span className="text-[#5c6478]">{formatInt(t.rows_estimate)}</span>
                      </>
                    )}
                  </span>
                </div>
              );
            })}
          </div>

          <div className="shrink-0 border-t border-[var(--color-border)] px-3 py-1.5 text-[11px] text-[var(--color-muted)]">
            已选 <b className="mx-0.5 text-[var(--color-text)]">{selected.size}</b> 张 ·{" "}
            <b className="text-[var(--color-text)]">{formatInt(totalRows)}</b> 行
          </div>
        </>
      ) : (
        <>
          <div className="min-h-0 flex-1 overflow-y-auto p-3">
            {!schema ? (
              <p className="text-[11.5px] leading-relaxed text-[var(--color-muted)]">
                选择目标库。库不存在时可自动创建。
              </p>
            ) : willCreate + existing === 0 ? (
              <p className="text-[11.5px] leading-relaxed text-[var(--color-muted)]">
                库里还没有对应的表。预检后会显示哪些表将被创建。
              </p>
            ) : (
              <div className="space-y-1.5 text-[11.5px]">
                {existing > 0 && (
                  <p className="text-[var(--color-muted)]">
                    已有 <b className="text-[var(--color-text)]">{existing}</b> 张目标表（只补数据，不改结构）
                  </p>
                )}
                {willCreate > 0 && (
                  <p className="text-[var(--color-accent)]">
                    将新建 <b>{willCreate}</b> 张表
                  </p>
                )}
              </div>
            )}
          </div>
          <div className="shrink-0 border-t border-[var(--color-border)] px-3 py-1.5 text-[11px] text-[var(--color-muted)]">
            写入 <b className="text-[var(--color-text)]">UPSERT</b> · 可重复执行
          </div>
        </>
      )}
    </div>
  );
}
