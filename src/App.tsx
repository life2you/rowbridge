import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "./lib/ipc";
import type {
  ConnForm,
  OptionsForm,
  PreflightReport,
  RunRequest,
  SchemaInfo,
  SyncEvent,
} from "./lib/types";
import { cn, formatInt } from "./lib/format";
import {
  ConnectionEditor,
  newConnection,
  type ConnectionDraft,
} from "./components/ConnectionEditor";
import { SchemaTree, useTables } from "./components/SchemaTree";
import { PreflightPanel } from "./components/PreflightPanel";
import { RunPanel, emptyRun, reduceRun, type RunState } from "./components/RunPanel";
import { Badge, Button, Checkbox, Empty, Field, Spinner } from "./components/ui";
import { HistoryPanel } from "./components/HistoryPanel";

type Step = 1 | 2 | 3;
type BottomTab = "preflight" | "run" | "history";

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
  const [step, setStep] = useState<Step>(1);
  const [src, setSrc] = useState<ConnectionDraft>(() => newConnection("src"));
  const [dst, setDst] = useState<ConnectionDraft>(() => newConnection("dst"));

  // 源和目标各自的库列表。曾经共用一个，导致目标下拉里列的是源连接的库。
  const [srcSchemas, setSrcSchemas] = useState<SchemaInfo[]>([]);
  const [dstSchemas, setDstSchemas] = useState<SchemaInfo[]>([]);
  const [loadingSchemas, setLoadingSchemas] = useState(false);
  const [srcSchema, setSrcSchema] = useState<string | null>(null);
  const [dstSchema, setDstSchema] = useState<string | null>(null);

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [options, setOptions] = useState<OptionsForm>(defaultOptions);

  const [report, setReport] = useState<PreflightReport | null>(null);
  const [preflighting, setPreflighting] = useState(false);
  const [preflightError, setPreflightError] = useState<string | null>(null);

  const [run, setRun] = useState<RunState>(emptyRun);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [bottomTab, setBottomTab] = useState<BottomTab>("preflight");

  const srcConn = toConnForm(src);
  const dstConn = toConnForm(dst);
  const { tables, loading: loadingTables } = useTables(srcSchema ? srcConn : null, srcSchema);

  // ---- 数据加载 ----
  useEffect(() => {
    let cancelled = false;
    setLoadingSchemas(true);
    api
      .listSchemas(srcConn)
      .then((s) => {
        if (!cancelled) setSrcSchemas(s);
      })
      .catch(() => {
        if (!cancelled) setSrcSchemas([]);
      })
      .finally(() => {
        if (!cancelled) setLoadingSchemas(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src.host, src.port, src.username, src.password, src.passwordSaved]);

  useEffect(() => {
    let cancelled = false;
    api
      .listSchemas(dstConn)
      .then((s) => {
        if (!cancelled) setDstSchemas(s);
      })
      .catch(() => {
        if (!cancelled) setDstSchemas([]);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dst.host, dst.port, dst.username, dst.password, dst.passwordSaved]);

  // 选了源库后，目标库默认同名（仅当目标端确实有同名库时）
  useEffect(() => {
    if (!srcSchema) return;
    if (dstSchema) return;
    if (dstSchemas.some((s) => s.name === srcSchema)) setDstSchema(srcSchema);
  }, [srcSchema, dstSchema, dstSchemas]);

  // 源库/表变化后，清掉过期的预检结果
  useEffect(() => {
    setReport(null);
    setPreflightError(null);
  }, [srcSchema, dstSchema, selected, options]);

  // ---- 选中逻辑 ----
  const toggleTable = (name: string) => {
    setSelected((prev) => {
      const n = new Set(prev);
      n.has(name) ? n.delete(name) : n.add(name);
      return n;
    });
  };
  const selectAll = (names: string[]) => setSelected(new Set(names));

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
  const syncableCount = report ? report.tables.length - blockingCount : 0;
  const canStart = !!report && syncableCount > 0 && !run.running && !preflighting;

  const buildRequest = (): RunRequest => ({
    source: srcConn,
    target: dstConn,
    src_schema: srcSchema!,
    dst_schema: dstSchema!,
    tables: selectedList.map((t) => ({ table: t.name, dst_table: null })),
    options,
  });

  // ---- 动作 ----
  async function doPreflight() {
    setPreflighting(true);
    setPreflightError(null);
    try {
      const r = await api.preflight(buildRequest());
      setReport(r);
      setBottomTab("preflight");
      // 预检结果只在第 3 步渲染；不跳转的话用户点了预检却什么都看不到。
      setStep(3);
    } catch (e) {
      setPreflightError(errorMessage(e));
      setReport(null);
    } finally {
      setPreflighting(false);
    }
  }

  async function doStart() {
    if (!report) return;
    const id = `task_${Date.now()}`;
    setTaskId(id);
    setStartError(null);
    setBottomTab("run");

    // 先重置，避免上一次的残留
    let state = reduceRun(emptyRun(), { type: "task_started", tables: report.tables.length }, report.tables);
    setRun(state);

    try {
      const res = await api.startSync(buildRequest(), id, report, (ev: SyncEvent) => {
        state = reduceRun(state, ev, report.tables);
        setRun(state);
      });
      // startSync 结束时兜底刷新一次
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
    if (!taskId) return;
    await api.cancelSync(taskId);
  }

  // ---- 渲染 ----
  return (
    <div className="flex h-full flex-col">
      {/* 顶栏：透明标题栏下留出 macOS 红绿灯的位置 */}
      <header
        data-tauri-drag-region
        className="flex h-11 shrink-0 items-center gap-3 border-b border-[var(--color-border)] bg-[var(--color-panel)] pl-[78px] pr-3"
      >
        <div className="flex items-center gap-2" data-tauri-drag-region>
          <span className="text-[13px] font-semibold tracking-tight">RowBridge</span>
          <span className="text-[10px] text-[var(--color-muted)]">MySQL 同步</span>
        </div>

        <nav className="ml-4 flex items-center gap-1">
          {([1, 2, 3] as Step[]).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setStep(s)}
              className={cn(
                "rounded px-2.5 py-1 text-[12px] transition-colors",
                step === s
                  ? "bg-[#232a38] text-[var(--color-text)]"
                  : "text-[var(--color-muted)] hover:text-[var(--color-text)]",
              )}
            >
              <span className="mr-1 opacity-50">{s}</span>
              {s === 1 ? "连接与选表" : s === 2 ? "同步选项" : "预检与执行"}
            </button>
          ))}
        </nav>

        <div className="ml-auto flex items-center gap-2">
          {run.running && <Badge tone="accent">同步中</Badge>}
          <button
            type="button"
            onClick={() => setBottomTab("history")}
            className="text-[12px] text-[var(--color-muted)] hover:text-[var(--color-text)]"
          >
            历史
          </button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        {/* 左：库表树 */}
        <aside className="flex w-[286px] shrink-0 flex-col border-r border-[var(--color-border)] bg-[var(--color-panel)]">
          <div className="space-y-2 border-b border-[var(--color-border)] p-3">
            <ConnectionEditor
              value={src}
              onChange={setSrc}
              title="源连接 A"
              accent="accent"
              testid="src"
            />
            <div className="flex justify-center text-[11px] text-[var(--color-muted)]">
              ↓ 数据流向
            </div>
            <ConnectionEditor
              value={dst}
              onChange={setDst}
              title="目标连接 B"
              accent="ok"
              testid="dst"
            />
          </div>
          <SchemaTree
            conn={src.passwordSaved ? srcConn : null}
            side="src"
            schemas={srcSchemas}
            loadingSchemas={loadingSchemas || loadingTables}
            selectedSchema={srcSchema}
            onSelectSchema={setSrcSchema}
            tables={tables}
            selectedTables={selected}
            onToggleTable={toggleTable}
            onSelectAll={selectAll}
            disabled={run.running}
          />
        </aside>

        {/* 右：主区 */}
        <main className="flex min-w-0 flex-1 flex-col">
          {step === 1 && (
            <div className="flex min-h-0 flex-1">
              <div className="flex min-w-0 flex-1 flex-col">
                <div className="flex items-center gap-3 border-b border-[var(--color-border)] px-4 py-2.5">
                  <span className="text-[12.5px] font-semibold">已选表</span>
                  <span className="text-[12px] text-[var(--color-muted)]">
                    {selectedList.length} 张 · 约 {formatInt(totalRows)} 行
                  </span>
                  <div className="ml-auto flex items-center gap-2">
                    <Button onClick={doPreflight} disabled={!canPreflight} testid="preflight">
                      {preflighting && <Spinner />}
                      {preflighting ? "预检中…" : "预检"}
                    </Button>
                    <Button
                      variant="primary"
                      onClick={() => setStep(3)}
                      disabled={selectedList.length === 0}
                      testid="next"
                    >
                      下一步
                    </Button>
                  </div>
                </div>

                <div className="min-h-0 flex-1 overflow-y-auto">
                  {selectedList.length === 0 ? (
                    <Empty>在左侧勾选要同步的表</Empty>
                  ) : (
                    <table className="w-full border-collapse text-[12.5px]">
                      <thead className="sticky top-0 bg-[var(--color-panel)] text-[11px] uppercase tracking-wide text-[var(--color-muted)]">
                        <tr className="border-b border-[var(--color-border)]">
                          <th className="px-4 py-2 text-left font-medium">表</th>
                          <th className="px-2 py-2 text-left font-medium">主键</th>
                          <th className="px-2 py-2 text-right font-medium">行数(估算)</th>
                          <th className="px-2 py-2 text-left font-medium">筛选</th>
                          <th className="px-4 py-2" />
                        </tr>
                      </thead>
                      <tbody>
                        {selectedList.map((t) => (
                          <tr
                            key={t.name}
                            className="border-b border-[#1e222b] hover:bg-[#151922]"
                          >
                            <td className="px-4 py-2">
                              <span className="font-medium">{t.name}</span>
                            </td>
                            <td className="px-2 py-2 mono text-[var(--color-muted)]">
                              {t.primary_key ?? "—"}
                            </td>
                            <td className="px-2 py-2 text-right mono">
                              {formatInt(t.rows_estimate)}
                            </td>
                            <td className="px-2 py-2 text-[11px] text-[var(--color-muted)]">
                              {options.end_id ? `≤ ${options.end_id}` : "全量"}
                            </td>
                            <td className="px-4 py-2 text-right">
                              <button
                                type="button"
                                onClick={() => toggleTable(t.name)}
                                className="text-[11px] text-[var(--color-muted)] hover:text-[var(--color-err)]"
                              >
                                移除
                              </button>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  )}
                </div>
              </div>

              {/* 目标侧参考 */}
              <div className="flex w-[240px] shrink-0 flex-col border-l border-[var(--color-border)] bg-[var(--color-panel)]">
                <div className="border-b border-[var(--color-border)] px-3 py-2 text-[11px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
                  目标库
                </div>
                <div className="p-3">
                  <Field label="目标 schema">
                    <select
                      data-testid="dst-schema"
                      value={dstSchema ?? ""}
                      onChange={(e) => setDstSchema(e.target.value || null)}
                      className="w-full"
                    >
                      <option value="">— 请选择 —</option>
                      {dstSchemas.map((s) => (
                        <option key={s.name} value={s.name}>
                          {s.name}
                        </option>
                      ))}
                      {dstSchema && !dstSchemas.some((s) => s.name === dstSchema) && (
                        <option value={dstSchema}>{dstSchema}（将自动创建）</option>
                      )}
                    </select>
                  </Field>
                  <p className="mt-2 text-[11px] leading-relaxed text-[var(--color-muted)]">
                    目标库不存在时会自动创建；表会按源表结构自动建。
                  </p>
                </div>
              </div>
            </div>
          )}

          {step === 2 && (
            <OptionsForm
              options={options}
              onChange={setOptions}
              onBack={() => setStep(1)}
              onNext={() => setStep(3)}
            />
          )}

          {step === 3 && (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex items-center gap-3 border-b border-[var(--color-border)] px-4 py-2.5">
                <button
                  type="button"
                  onClick={() => setStep(1)}
                  className="text-[12px] text-[var(--color-muted)] hover:text-[var(--color-text)]"
                >
                  ← 返回
                </button>
                <Button onClick={doPreflight} disabled={!canPreflight} testid="repreflight">
                  {preflighting && <Spinner />}
                  {preflighting ? "预检中…" : "重新预检"}
                </Button>
                <span className="ml-auto flex items-center gap-2">
                  {report && blockingCount > 0 && (
                    <Badge tone="err">{blockingCount} 张表被阻塞</Badge>
                  )}
                  {report && syncableCount > 0 && (
                    <Badge tone="ok">{syncableCount} 张可同步</Badge>
                  )}
                  <Button
                    variant="primary"
                    onClick={doStart}
                    testid="start"
                    disabled={!canStart}
                    title={
                      !report
                        ? "请先预检"
                        : syncableCount === 0
                          ? "没有可同步的表"
                          : undefined
                    }
                  >
                    开始同步
                  </Button>
                </span>
              </div>

              {preflightError && (
                <div className="m-3 rounded border border-[#5c2a2f] bg-[#2a1619] p-2.5 text-[12px] text-[#ffb3ae] break-words">
                  {preflightError}
                </div>
              )}
              {startError && (
                <div className="m-3 rounded border border-[#5c2a2f] bg-[#2a1619] p-2.5 text-[12px] text-[#ffb3ae] break-words">
                  {startError}
                </div>
              )}

              <div className="min-h-0 flex-1">
                {bottomTab === "run" ? (
                  <RunPanel run={run} tables={report?.tables ?? []} onCancel={doCancel} />
                ) : bottomTab === "history" ? (
                  <HistoryPanel />
                ) : (
                  <PreflightPanel report={report} />
                )}
              </div>
            </div>
          )}
        </main>
      </div>
    </div>
  );
}

function OptionsForm({
  options,
  onChange,
  onBack,
  onNext,
}: {
  options: OptionsForm;
  onChange: (o: OptionsForm) => void;
  onBack: () => void;
  onNext: () => void;
}) {
  const set = <K extends keyof OptionsForm>(k: K, v: OptionsForm[K]) =>
    onChange({ ...options, [k]: v });

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-4 py-2.5">
        <span className="text-[12.5px] font-semibold">同步选项</span>
        <span className="ml-auto flex gap-2">
          <Button onClick={onBack}>上一步</Button>
          <Button variant="primary" onClick={onNext}>
            下一步
          </Button>
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <div className="grid max-w-3xl gap-4">
          <Section title="性能">
            <div className="grid grid-cols-3 gap-3">
              <Field label="并行表数" hint="同时同步几张表">
                <input
                  type="number"
                  min={1}
                  max={16}
                  value={options.parallelism}
                  onChange={(e) => set("parallelism", Number(e.target.value) || 1)}
                />
              </Field>
              <Field label="初始批大小" hint="每批读取的行数">
                <input
                  type="number"
                  min={1}
                  value={options.batch_size}
                  onChange={(e) => set("batch_size", Number(e.target.value) || 1)}
                />
              </Field>
              <Field label="批大小上限">
                <input
                  type="number"
                  min={1}
                  value={options.max_batch_size}
                  onChange={(e) => set("max_batch_size", Number(e.target.value) || 1)}
                />
              </Field>
            </div>
            <div className="mt-2">
              <Checkbox
                checked={options.adaptive_batch}
                onChange={(v) => set("adaptive_batch", v)}
                label="自适应调整批大小"
                hint="耗时短则倍增，超时则减半"
              />
            </div>
          </Section>

          <Section title="同步范围">
            <div className="grid grid-cols-3 gap-3">
              <Field label="起始游标">
                <select
                  value={options.cursor_start}
                  onChange={(e) =>
                    set("cursor_start", e.target.value as OptionsForm["cursor_start"])
                  }
                >
                  <option value="full">全量（从最小 id 开始）</option>
                  <option value="resume">续传（从目标 MAX(id) 之后）</option>
                  <option value="custom">自定义</option>
                </select>
              </Field>
              <Field label="自定义起始 id" hint="选「自定义」时生效">
                <input
                  value={options.start_id ?? ""}
                  disabled={options.cursor_start !== "custom"}
                  onChange={(e) => set("start_id", e.target.value || null)}
                  placeholder="例如 100000"
                  spellCheck={false}
                />
              </Field>
              <Field label="结束 id（含）" hint="留空表示同步到最后">
                <input
                  value={options.end_id ?? ""}
                  onChange={(e) => set("end_id", e.target.value || null)}
                  placeholder="不限"
                  spellCheck={false}
                />
              </Field>
            </div>
          </Section>

          <Section title="目标表处理">
            <div className="grid gap-2">
              <Checkbox
                checked={options.create_table}
                onChange={(v) => set("create_table", v)}
                label="目标表不存在时，按源表结构自动创建"
              />
              <Checkbox
                checked={options.create_database}
                onChange={(v) => set("create_database", v)}
                label="目标库不存在时自动创建"
              />
              <Checkbox
                checked={options.strip_foreign_keys}
                onChange={(v) => set("strip_foreign_keys", v)}
                label="建表时移除外键约束"
                hint="避免被引用表尚未创建导致建表失败"
              />
              <Checkbox
                checked={options.strip_auto_increment}
                onChange={(v) => set("strip_auto_increment", v)}
                label="建表时移除 AUTO_INCREMENT 起始值"
                hint="主键值由数据显式写入，保留会导致空洞"
              />
              <Checkbox
                checked={options.map_collation}
                onChange={(v) => set("map_collation", v)}
                label="跨版本时自动降级 collation"
                hint="如 utf8mb4_0900_ai_ci → utf8mb4_unicode_ci"
              />
            </div>
          </Section>

          <Section title="高级">
            <div className="grid gap-2">
              <Checkbox
                checked={options.continue_on_error}
                onChange={(v) => set("continue_on_error", v)}
                label="单表失败后继续同步其它表"
              />
              <Checkbox
                checked={options.disable_foreign_key_checks}
                onChange={(v) => set("disable_foreign_key_checks", v)}
                label="目标会话关闭外键检查（foreign_key_checks=0）"
                hint="可能写入孤儿行，需自行确保数据完整"
              />
              <Checkbox
                checked={options.relax_sql_mode}
                onChange={(v) => set("relax_sql_mode", v)}
                label="目标会话放宽 sql_mode"
                hint="用于兼容零日期等历史脏数据，可能静默截断"
              />
            </div>
          </Section>

          <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] p-3 text-[11.5px] leading-relaxed text-[var(--color-muted)]">
            写入策略固定为 <span className="text-[var(--color-text)]">UPSERT</span>
            （INSERT … ON DUPLICATE KEY UPDATE）：可重复执行，中断后直接重跑即可。
            首版不做 DELETE 同步，目标端多出的行会保留。
          </div>
        </div>
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] p-3.5">
      <h3 className="mb-2.5 text-[11px] font-semibold uppercase tracking-wide text-[var(--color-muted)]">
        {title}
      </h3>
      {children}
    </section>
  );
}
