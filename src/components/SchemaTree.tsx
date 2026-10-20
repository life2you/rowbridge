import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import { formatInt } from "../lib/format";
import type { ConnForm, SchemaInfo, TableRow } from "../lib/types";
import { Badge, Spinner } from "./ui";
import { cn } from "../lib/format";

/** 左侧：库 → 表 的树。 */
export function SchemaTree({
  conn,
  side,
  schemas,
  loadingSchemas,
  selectedSchema,
  onSelectSchema,
  tables,
  selectedTables,
  onToggleTable,
  onSelectAll,
  disabled,
}: {
  conn: ConnForm | null;
  side: "src" | "dst";
  schemas: SchemaInfo[];
  loadingSchemas: boolean;
  selectedSchema: string | null;
  onSelectSchema: (s: string) => void;
  tables: TableRow[];
  selectedTables: Set<string>;
  onToggleTable: (name: string) => void;
  onSelectAll: (names: string[]) => void;
  disabled: boolean;
}) {
  const [expanded, setExpanded] = useState(true);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    setFilter("");
  }, [selectedSchema]);

  const visible = tables.filter((t) =>
    t.name.toLowerCase().includes(filter.toLowerCase()),
  );
  const syncableNames = visible
    .filter((t) => !t.unsupported_reason && !t.is_view)
    .map((t) => t.name);
  const allSelected =
    syncableNames.length > 0 && syncableNames.every((n) => selectedTables.has(n));

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between border-b border-[var(--color-border)] px-3 py-2">
        <span className="text-[11px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
          {side === "src" ? "源 · 选择表" : "目标 · 参照"}
        </span>
        {loadingSchemas && <Spinner className="text-[var(--color-muted)]" />}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {schemas.length === 0 && !loadingSchemas && (
          <div className="px-3 py-4 text-[12px] text-[var(--color-muted)]">
            {conn ? "没有可用的库" : "请先在上方配置并测试连接"}
          </div>
        )}

        {schemas.map((s) => {
          const active = s.name === selectedSchema;
          return (
            <div key={s.name}>
              <button
                type="button"
                data-testid={`schema-${s.name}`}
                disabled={disabled}
                onClick={() => {
                  onSelectSchema(s.name);
                  setExpanded(true);
                }}
                className={cn(
                  "flex w-full items-center gap-1.5 px-2 py-1.5 text-left text-[13px] transition-colors",
                  active
                    ? "bg-[#1d2534] text-[var(--color-text)]"
                    : "text-[var(--color-muted)] hover:bg-[#1a1e27]",
                  disabled && "opacity-50 cursor-not-allowed",
                )}
              >
                <span className="text-[10px] opacity-60">{active ? "▾" : "▸"}</span>
                <span className="truncate">{s.name}</span>
                {s.collation && (
                  <span className="ml-auto shrink-0 text-[10px] opacity-50">
                    {s.collation.replace("utf8mb4_", "")}
                  </span>
                )}
              </button>

              {active && expanded && (
                <div className="border-l border-[var(--color-border)] pl-1">
                  <div className="px-2 py-1.5">
                    <input
                      value={filter}
                      onChange={(e) => setFilter(e.target.value)}
                      placeholder="过滤表名…"
                      className="w-full !py-1 !text-[12px]"
                      spellCheck={false}
                    />
                  </div>

                  {side === "src" && visible.length > 0 && (
                    <div className="flex items-center justify-between px-2 pb-1">
                      <button
                        type="button"
                        onClick={() =>
                          onSelectAll(allSelected ? [] : syncableNames)
                        }
                        className="text-[11px] text-[var(--color-accent)] hover:underline"
                      >
                        {allSelected ? "取消全选" : `全选可同步 (${syncableNames.length})`}
                      </button>
                      <span className="text-[11px] text-[var(--color-muted)]">
                        已选 {[...selectedTables].filter((n) => tables.some((t) => t.name === n)).length}
                      </span>
                    </div>
                  )}

                  {visible.map((t) => {
                    const unsupported = !!t.unsupported_reason || t.is_view;
                    const checked = selectedTables.has(t.name);
                    return (
                      <div
                        key={t.name}
                        title={t.unsupported_reason ?? undefined}
                        className={cn(
                          "flex items-center gap-2 px-2 py-[5px] text-[12.5px]",
                          unsupported && "opacity-45",
                          !unsupported && "hover:bg-[#1a1e27]",
                        )}
                      >
                        {side === "src" ? (
                          <input
                            type="checkbox"
                            data-testid={`table-${t.name}`}
                            checked={checked}
                            disabled={unsupported}
                            onChange={() => onToggleTable(t.name)}
                            className="h-3.5 w-3.5 shrink-0 accent-[var(--color-accent)]"
                          />
                        ) : (
                          <span className="w-3.5 shrink-0" />
                        )}
                        <span className="truncate">{t.name}</span>
                        <span className="ml-auto flex shrink-0 items-center gap-1.5">
                          {t.primary_key && (
                            <span className="text-[10px] text-[var(--color-muted)]">
                              {t.primary_key}
                            </span>
                          )}
                          {unsupported ? (
                            <Badge tone="err">
                              {t.is_view ? "视图" : "不支持"}
                            </Badge>
                          ) : (
                            <span className="text-[10px] text-[#5c6478]">
                              {formatInt(t.rows_estimate)}
                            </span>
                          )}
                        </span>
                      </div>
                    );
                  })}
                  {visible.length === 0 && filter && (
                    <div className="px-3 py-2 text-[11px] text-[var(--color-muted)]">
                      无匹配
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** 供其它组件复用的表列表加载 hook。 */
export function useTables(conn: ConnForm | null, schema: string | null) {
  const [tables, setTables] = useState<TableRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!conn || !schema) {
      setTables([]);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .listTables(conn, schema)
      .then((t) => {
        if (!cancelled) setTables(t);
      })
      .catch((e) => {
        if (!cancelled) {
          setError(errorMessage(e));
          setTables([]);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [conn, schema]);

  return { tables, loading, error };
}
