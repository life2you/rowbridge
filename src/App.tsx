import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage } from "./lib/ipc";
import type {
  ConnForm,
  ValidatedTable,
  SavedConnection,
  OptionsForm,
  PreflightReport,
  RunRequest,
  SchemaInfo,
  SyncEvent,
  TableRow,
} from "./lib/types";
import { cn } from "./lib/format";
import { emptyRun, reduceRun, type RunState } from "./lib/run";
import type { TableCursor } from "./lib/cursor";
import {
  newConnection,
  type ConnectionDraft,
} from "./components/ConnectionEditor";
import { TablePane } from "./components/TablePane";
import { BatchView } from "./components/BatchView";
import { DataSourceManager } from "./components/DataSourceManager";
import { RunDrawer } from "./components/RunDrawer";
import { HistoryPanel } from "./components/HistoryPanel";
import { Badge } from "./components/ui";

const defaultOptions: OptionsForm = {
  parallelism: 2,
  batch_size: 1000,
  max_batch_size: 10000,
  adaptive_batch: true,
  cursor_start: "full",
  start_id: null,
  end_id: null,
  create_table: true,
  create_database: true,
  strip_foreign_keys: true,
  strip_auto_increment: true,
  map_collation: true,
  disable_foreign_key_checks: false,
  relax_sql_mode: false,
  continue_on_error: true,
};

function toConnForm(c: ConnectionDraft): ConnForm {
  return {
    host: c.host,
    port: c.port,
    username: c.username,
    password: c.password || null,
    connection_id: c.passwordSaved ? c.id : null,
    use_tls: c.use_tls,
  };
}

export default function App() {
  const [src, setSrc] = useState<ConnectionDraft>(() => newConnection("src"));
  const [dst, setDst] = useState<ConnectionDraft>(() => newConnection("dst"));

  const [srcSchemas, setSrcSchemas] = useState<SchemaInfo[]>([]);
  const [dstSchemas, setDstSchemas] = useState<SchemaInfo[]>([]);
  const [srcSchema, setSrcSchema] = useState<string | null>(null);
  const [dstSchema, setDstSchema] = useState<string | null>(null);
  const [loadingTables, setLoadingTables] = useState(false);
  const [tables, setTables] = useState<TableRow[]>([]);

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [options, setOptions] = useState<OptionsForm>(defaultOptions);
  // 每张表的游标覆写。缺省 = 跟随 options 里的全局设置。
  const [overrides, setOverrides] = useState<Record<string, TableCursor>>({});

  const [report, setReport] = useState<PreflightReport | null>(null);
  const [preflighting, setPreflighting] = useState(false);
  const [preflightError, setPreflightError] = useState<string | null>(null);

  const [savedConns, setSavedConns] = useState<SavedConnection[]>([]);
  // 已通过 validate_tables 校验的表：表名 -> 校验结果
  const [validated, setValidated] = useState<Record<string, ValidatedTable>>({});
  const [validating, setValidating] = useState<Set<string>>(new Set());
  // 串行队列：连勾多张时依次校验（后端限流 3，这里再限前端每批最多 3）
  const validateQueueRef = useRef<string[]>([]);


  const [run, setRun] = useState<RunState>(emptyRun);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [showDataSources, setShowDataSources] = useState(false);

  const srcConn = toConnForm(src);
  const dstConn = toConnForm(dst);

  // ---- 启动时加载已保存的数据源 ----
  useEffect(() => {
    api
      .listConnections()
      .then(setSavedConns)
      .catch(() => {});
  }, []);

  // ---- 库列表 ----
  useEffect(() => {
    let dead = false;
    api
      .listSchemas(srcConn)
      .then((s) => !dead && setSrcSchemas(s))
      .catch(() => !dead && setSrcSchemas([]));
    return () => {
      dead = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src.host, src.port, src.username, src.password, src.passwordSaved]);

  useEffect(() => {
    let dead = false;
    api
      .listSchemas(dstConn)
      .then((s) => !dead && setDstSchemas(s))
      .catch(() => !dead && setDstSchemas([]));
    return () => {
      dead = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dst.host, dst.port, dst.username, dst.password, dst.passwordSaved]);

  // ---- 表列表 ----
  useEffect(() => {
    if (!srcSchema) {
      setTables([]);
      return;
    }
    let dead = false;
    setLoadingTables(true);
    api
      .listTables(srcConn, srcSchema)
      .then((t) => !dead && setTables(t))
      .catch(() => !dead && setTables([]))
      .finally(() => !dead && setLoadingTables(false));
    return () => {
      dead = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [srcSchema, src.host, src.port, src.username, src.password, src.passwordSaved]);

  // 目标库默认选同名（若目标端存在）
  useEffect(() => {
    if (!srcSchema || dstSchema) return;
    if (dstSchemas.some((s) => s.name === srcSchema)) setDstSchema(srcSchema);
  }, [srcSchema, dstSchema, dstSchemas]);

  // 改动后预检结果失效
  useEffect(() => {
    setReport(null);
    setPreflightError(null);
  }, [srcSchema, dstSchema, selected, options, overrides]);

  // ---- 数据源操作 ----
  function pickSaved(side: "src" | "dst", c: SavedConnection) {
    const apply = (prev: ConnectionDraft): ConnectionDraft => ({
      ...prev,
      id: c.id,
      label: c.name,
      host: c.host,
      port: c.port,
      username: c.username,
      use_tls: c.use_tls,
      // 密码在钥匙串里，留空表示沿用
      password: "",
      passwordSaved: true,
      connection_id: c.id,
    });
    if (side === "src") {
      setSrc(apply);
      if (c.last_schema) setSrcSchema(c.last_schema);
    } else {
      setDst(apply);
      if (c.last_schema) setDstSchema(c.last_schema);
    }
  }

  // ---- 选择 ----
  const toggleTable = (name: string) => {
    setSelected((prev) => {
      const n = new Set(prev);
      if (n.has(name)) {
        n.delete(name);
      } else {
        n.add(name);
        // 勾选后异步校验这张表（重活懒加载）
        enqueueValidate(name);
      }
      return n;
    });
  };

  const selectAll = (names: string[]) => {
    setSelected(new Set(names));
    for (const n of names) enqueueValidate(n);
  };

  // 校验队列：防抖合并，最多同时发 3 个请求
  const enqueueValidate = (name: string) => {
    if (validated[name] || validating.has(name)) return;
    validateQueueRef.current.push(name);
    scheduleValidate();
  };

  const validateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scheduleValidate = () => {
    if (validateTimerRef.current) return;
    validateTimerRef.current = setTimeout(async () => {
      validateTimerRef.current = null;
      const batch = validateQueueRef.current
        .filter((n) => !validated[n] && !validating.has(n))
        .slice(0, 3);
      validateQueueRef.current = [];
      if (batch.length === 0) return;
      setValidating((prev) => new Set([...prev, ...batch]));
      try {
        const results = await api.validateTables(srcConn, srcSchema!, batch);
        setValidated((prev) => {
          const next = { ...prev };
          for (const r of results) next[r.name] = r;
          return next;
        });
        // 若校验发现不可同步（列级问题），自动取消勾选
        setSelected((prev) => {
          const n = new Set(prev);
          for (const r of results) {
            if (r.unsupported_reason) n.delete(r.name);
          }
          return n;
        });
      } catch {
        /* 校验失败保持现状，预检会再查 */
      } finally {
        setValidating((prev) => {
          const next = new Set(prev);
          for (const n of batch) next.delete(n);
          return next;
        });
        // 队列里可能还有新的，继续
        if (validateQueueRef.current.length > 0) scheduleValidate();
      }
    }, 150);
  };

  const selectedList = useMemo(
    () => tables.filter((t) => selected.has(t.name)),
    [tables, selected],
  );
  const totalRows = useMemo(
    () => selectedList.reduce((s, t) => s + (t.rows_estimate || 0), 0),
    [selectedList],
  );
  const canPreflight =
    !!srcSchema && !!dstSchema && selectedList.length > 0 && !preflighting;
  const blockingCount = report
    ? report.tables.filter((t) => !t.syncable).length
    : 0;
  const runnableCount = report ? report.tables.length - blockingCount : 0;
  const canStart = !!report && runnableCount > 0 && !run.running && !preflighting;

  const buildRequest = (): RunRequest => ({
    source: srcConn,
    target: dstConn,
    src_schema: srcSchema!,
    dst_schema: dstSchema!,
    tables: selectedList.map((t) => {
      const ov = overrides[t.name] ?? {};
      return {
        table: t.name,
        dst_table: null,
        cursor_start: ov.cursor_start ?? null,
        start_id: ov.start_id ?? null,
        end_id: ov.end_id ?? null,
      };
    }),
    options,
  });

  // ---- 动作 ----
  async function doPreflight() {
    setPreflighting(true);
    setPreflightError(null);
    try {
      const r = await api.preflight(buildRequest());
      setReport(r);
      setDrawerOpen(true);
    } catch (e) {
      setPreflightError(errorMessage(e));
      setReport(null);
      setDrawerOpen(true);
    } finally {
      setPreflighting(false);
    }
  }

  async function doStart() {
    if (!report) return;
    const id = `task_${Date.now()}`;
    setTaskId(id);
    setStartError(null);
    setDrawerOpen(true);

    let state = reduceRun(
      emptyRun(),
      { type: "task_started", tables: report.tables.length },
      report.tables,
    );
    setRun(state);

    try {
      const res = await api.startSync(buildRequest(), id, report, (ev: SyncEvent) => {
        state = reduceRun(state, ev, report.tables);
        setRun(state);
      });
      setRun((prev) => ({
        ...prev,
        running: false,
        finished: true,
        summary: {
          totalRows: res.total_rows,
          succeeded: res.succeeded,
          failed: res.failed,
          partial: res.partial,
          elapsedMs: res.elapsed_ms,
        },
        cancelled: res.cancelled,
      }));
    } catch (e) {
      setStartError(errorMessage(e));
      setRun((prev) => ({ ...prev, running: false, finished: true }));
    }
  }

  async function doCancel() {
    if (taskId) await api.cancelSync(taskId);
  }

  return (
    <div className="flex h-full flex-col">
      {/* 顶栏 */}
      <header
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center gap-3 border-b border-[var(--color-border)] bg-[var(--color-panel)] pl-[78px] pr-3"
      >
        <span className="text-[13px] font-semibold tracking-tight">RowBridge</span>
        <span className="text-[10px] text-[var(--color-muted)]">MySQL 同步</span>

        {report && (
          <>
            <span className="mx-1 h-4 w-px bg-[var(--color-border)]" />
            {blockingCount > 0 ? (
              <Badge tone="err">{blockingCount} 张被阻塞</Badge>
            ) : (
              <Badge tone="ok">预检通过 {runnableCount}</Badge>
            )}
          </>
        )}

        <div className="ml-auto flex items-center gap-2">
          {run.running && <Badge tone="accent">同步中</Badge>}
          <button
            type="button"
            data-testid="nav-datasources"
            onClick={() => {
              setShowDataSources((v) => !v);
              setShowHistory(false);
            }}
            className={cn(
              "rounded px-2 py-1 text-[12px]",
              showDataSources
                ? "bg-[#232a38] text-[var(--color-text)]"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]",
            )}
          >
            数据源
          </button>
          <button
            type="button"
            onClick={() => {
              setShowHistory((v) => !v);
              setShowDataSources(false);
            }}
            className={cn(
              "rounded px-2 py-1 text-[12px]",
              showHistory
                ? "bg-[#232a38] text-[var(--color-text)]"
                : "text-[var(--color-muted)] hover:text-[var(--color-text)]",
            )}
          >
            历史
          </button>
        </div>
      </header>

      {showDataSources ? (
        <div className="min-h-0 flex-1">
          <DataSourceManager
            list={savedConns}
            onChange={setSavedConns}
            onClose={() => setShowDataSources(false)}
          />
        </div>
      ) : showHistory ? (
        <div className="min-h-0 flex-1">
          <HistoryPanel />
        </div>
      ) : (
        <>
          {/* 双栏主体 */}
          <div className="flex min-h-0 flex-1">
            <TablePane
              side="src"
              conn={src}
              schemas={srcSchemas}
              schema={srcSchema}
              onSchemaChange={setSrcSchema}
              tables={tables}
              loading={loadingTables}
              selected={selected}
              onToggle={toggleTable}
              onSelectAll={selectAll}
              totalRows={totalRows}
              disabled={run.running}
              validated={validated}
              validating={validating}
              saved={savedConns}
              onPickSaved={(c) => pickSaved("src", c)}
            />

            <div className="relative flex min-w-[560px] flex-1 flex-col">
              <BatchView
                rows={selectedList}
                overrides={overrides}
                onChange={(t, ov) =>
                  setOverrides((prev) => ({ ...prev, [t]: ov }))
                }
                global={{
                  cursor_start: options.cursor_start,
                  start_id: options.start_id ?? null,
                  end_id: options.end_id ?? null,
                }}
                onGlobalChange={(g) =>
                  setOptions({
                    ...options,
                    cursor_start: g.cursor_start,
                    start_id: g.start_id,
                    end_id: g.end_id,
                  })
                }
                batchSize={options.batch_size}
                maxBatchSize={options.max_batch_size}
                adaptiveBatch={options.adaptive_batch}
                onBatchChange={(batch, maxBatch, adaptive) =>
                  setOptions({
                    ...options,
                    batch_size: batch,
                    max_batch_size: maxBatch,
                    adaptive_batch: adaptive,
                  })
                }
                onRemove={toggleTable}
                onPreflight={doPreflight}
                onStart={doStart}
                preflighting={preflighting}
                canPreflight={canPreflight}
                canStart={canStart}
                running={run.running}
                hasReport={!!report}
              />
            </div>

            <TablePane
              side="dst"
              conn={dst}
              schemas={dstSchemas}
              schema={dstSchema}
              onSchemaChange={setDstSchema}
              tables={[]}
              selected={new Set()}
              onToggle={() => {}}
              onSelectAll={() => {}}
              totalRows={0}
              disabled={run.running}
              willCreate={report?.tables.filter((t) => t.needs_create_table).length ?? 0}
              existing={report?.tables.filter((t) => t.target_rows_exists).length ?? 0}
              saved={savedConns}
              onPickSaved={(c) => pickSaved("dst", c)}
              validated={{}}
              validating={new Set()}
            />
          </div>

          {/* 底部运行抽屉 */}
          <RunDrawer
            open={drawerOpen}
            onToggle={() => setDrawerOpen((v) => !v)}
            run={run}
            report={report}
            preflighting={preflighting}
            preflightError={preflightError}
            startError={startError}
            onCancel={doCancel}
            onPreflight={doPreflight}
            canPreflight={canPreflight}
          />
        </>
      )}
    </div>
  );
}
