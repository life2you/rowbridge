import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import type { SavedConnection } from "../lib/types";
import { Badge, Button, Spinner } from "./ui";
import { cn } from "../lib/format";

interface Draft {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  password: string;
  use_tls: boolean;
  /** 已保存的数据源密码从配置文件里读出来，直接填在表单里 */
  hasPassword: boolean;
}

function blank(): Draft {
  return {
    id: `conn_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
    name: "",
    host: "127.0.0.1",
    port: 3306,
    username: "root",
    password: "",
    use_tls: false,
    hasPassword: false,
  };
}

function fromSaved(c: SavedConnection): Draft {
  return {
    id: c.id,
    name: c.name,
    host: c.host,
    port: c.port,
    username: c.username,
    // 密码就在 connections.json 里，直接显示在表单
    password: c.password ?? "",
    use_tls: c.use_tls,
    hasPassword: !!c.password,
  };
}

/** 数据源管理：左侧列表，右侧表单，支持新增 / 编辑 / 删除 / 测试。 */
export function DataSourceManager({
  list,
  onChange,
  onClose,
}: {
  list: SavedConnection[];
  onChange: (list: SavedConnection[]) => void;
  onClose: () => void;
}) {
  const [editing, setEditing] = useState<Draft | null>(null);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testResult, setTestResult] = useState<
    { ok: true; version: string; schemas: number } | { ok: false; error: string } | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [configPath, setConfigPath] = useState("");

  useEffect(() => {
    api.connectionsPath().then(setConfigPath).catch(() => {});
  }, []);

  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => {
    setEditing((d) => (d ? { ...d, [k]: v } : d));
    setTestResult(null);
  };

  async function test() {
    if (!editing) return;
    setTesting(true);
    setTestResult(null);
    try {
      const r = await api.testConnection({
        host: editing.host,
        port: editing.port,
        username: editing.username,
        password: editing.password || null,
        connection_id: editing.hasPassword ? editing.id : null,
        use_tls: editing.use_tls,
      });
      setTestResult({ ok: true, version: r.version, schemas: r.schemas });
    } catch (e) {
      setTestResult({ ok: false, error: errorMessage(e) });
    } finally {
      setTesting(false);
    }
  }

  async function save() {
    if (!editing) return;
    if (!editing.name.trim()) {
      setError("请填一个名字，方便以后辨认");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const next = await api.saveConnection({
        id: editing.id,
        name: editing.name.trim(),
        host: editing.host.trim(),
        port: editing.port,
        username: editing.username.trim(),
        password: editing.password,
        use_tls: editing.use_tls,
        last_schema: null,
      });
      onChange(next);
      // 保存后保持编辑态，方便继续改
      setEditing((d) =>
        d ? { ...d, hasPassword: !!d.password } : d,
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  async function remove(id: string) {
    try {
      const next = await api.removeConnection(id);
      onChange(next);
      if (editing?.id === id) setEditing(null);
      setConfirmDelete(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  return (
    <div className="flex h-full min-h-0 bg-[var(--color-bg)]">
      {/* 列表 */}
      <div className="flex w-[300px] shrink-0 flex-col border-r border-[var(--color-border)] bg-[var(--color-panel)]">
        <div className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
          <span className="text-[12.5px] font-semibold">数据源</span>
          <Badge tone="muted">{list.length}</Badge>
          <Button
            onClick={() => {
              setEditing(blank());
              setTestResult(null);
              setError(null);
            }}
            className="ml-auto !py-1 !text-[11.5px]"
            testid="ds-add"
          >
            + 添加
          </Button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {list.length === 0 && (
            <div className="px-3 py-6 text-center text-[11.5px] leading-relaxed text-[var(--color-muted)]">
              还没有保存的数据源。
              <br />
              点右上角「+ 添加」创建第一个。
            </div>
          )}
          {list.map((c) => {
            const active = editing?.id === c.id;
            return (
              <div
                key={c.id}
                className={cn(
                  "flex items-center gap-2 border-b border-[#1e222b] px-3 py-2",
                  active ? "bg-[#17233a]" : "hover:bg-[#1a1e27]",
                )}
              >
                <button
                  type="button"
                  onClick={() => {
                    setEditing(fromSaved(c));
                    setTestResult(null);
                    setError(null);
                  }}
                  className="min-w-0 flex-1 text-left"
                >
                  <div className="truncate text-[12.5px] font-medium">{c.name}</div>
                  <div className="mono truncate text-[10.5px] text-[var(--color-muted)]">
                    {c.username}@{c.host}:{c.port}
                    {c.last_schema && ` · ${c.last_schema}`}
                  </div>
                </button>
                <button
                  type="button"
                  onClick={() => setConfirmDelete(c.id)}
                  className="shrink-0 rounded px-1.5 py-1 text-[11px] text-[var(--color-muted)] hover:text-[var(--color-err)]"
                  title="删除"
                >
                  删除
                </button>
              </div>
            );
          })}
        </div>

        <div className="shrink-0 border-t border-[var(--color-border)] px-3 py-1.5">
          <button
            type="button"
            onClick={() => api.revealConfigDir().catch(() => {})}
            className="mono w-full truncate text-left text-[10px] text-[#5c6478] hover:text-[var(--color-accent)]"
            title="在 Finder 中打开"
          >
            {configPath || "~/.RowBridge/connections.json"}
          </button>
        </div>
      </div>

      {/* 表单 */}
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex shrink-0 items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-panel)] px-3 py-2">
          <span className="text-[12.5px] font-semibold">
            {editing ? (list.some((c) => c.id === editing.id) ? "编辑数据源" : "新建数据源") : "数据源"}
          </span>
          <div className="ml-auto flex items-center gap-2">
            <Button onClick={onClose} className="!py-1" testid="ds-close">
              完成
            </Button>
          </div>
        </div>

        {!editing ? (
          <div className="flex flex-1 items-center justify-center px-6 text-center text-[12px] leading-relaxed text-[var(--color-muted)]">
            {list.length > 0 ? (
              <div>
                从左侧选一个数据源来编辑，
                <br />
                或点「+ 添加」新建。
              </div>
            ) : (
              <div>
                数据源保存在 <span className="mono text-[var(--color-text)]">~/.RowBridge/connections.json</span>
                <br />
                密码和配置放在一起，都在这个文件里。
              </div>
            )}
          </div>
        ) : (
          <div className="min-h-0 flex-1 overflow-y-auto p-4">
            <div className="grid max-w-[560px] gap-3">
              <Field label="名称" hint="比如「生产库只读」「测试环境」">
                <input
                  data-testid="ds-name"
                  value={editing.name}
                  onChange={(e) => set("name", e.target.value)}
                  placeholder="我的数据库"
                  spellCheck={false}
                />
              </Field>

              <div className="grid grid-cols-[1fr_100px] gap-2">
                <Field label="主机">
                  <input
                    data-testid="ds-host"
                    value={editing.host}
                    onChange={(e) => set("host", e.target.value)}
                    placeholder="127.0.0.1"
                    spellCheck={false}
                  />
                </Field>
                <Field label="端口">
                  <input
                    data-testid="ds-port"
                    type="number"
                    value={editing.port}
                    onChange={(e) => set("port", Number(e.target.value) || 3306)}
                  />
                </Field>
              </div>

              <div className="grid grid-cols-2 gap-2">
                <Field label="用户名">
                  <input
                    data-testid="ds-username"
                    value={editing.username}
                    onChange={(e) => set("username", e.target.value)}
                    spellCheck={false}
                  />
                </Field>
                <Field
                  label="密码"
                  hint={editing.hasPassword ? "已在配置文件中" : undefined}
                >
                  <input
                    data-testid="ds-password"
                    type="password"
                    value={editing.password}
                    onChange={(e) => set("password", e.target.value)}
                    placeholder=""
                  />
                </Field>
              </div>

              <label className="flex cursor-pointer items-center gap-2 text-[12px]">
                <input
                  type="checkbox"
                  checked={editing.use_tls}
                  onChange={(e) => set("use_tls", e.target.checked)}
                  className="h-3.5 w-3.5 accent-[var(--color-accent)]"
                />
                使用 TLS 连接
              </label>

              <div className="flex items-center gap-2">
                <Button
                  onClick={test}
                  disabled={testing}
                  testid="ds-test"
                >
                  {testing && <Spinner />}
                  {testing ? "测试中…" : "测试连接"}
                </Button>
                <Button
                  variant="primary"
                  onClick={save}
                  disabled={saving}
                  testid="ds-save"
                >
                  {saving && <Spinner />}
                  {list.some((c) => c.id === editing.id) ? "保存修改" : "保存"}
                </Button>
                {testResult?.ok && (
                  <Badge tone="ok">
                    v{testResult.version} · {testResult.schemas} 个库
                  </Badge>
                )}
              </div>

              {testResult && !testResult.ok && (
                <div className="break-words rounded border border-[#5c2a2f] bg-[#2a1619] px-2.5 py-2 text-[11.5px] text-[var(--color-err)]">
                  {testResult.error}
                </div>
              )}
              {error && (
                <div className="break-words rounded border border-[#5c2a2f] bg-[#2a1619] px-2.5 py-2 text-[11.5px] text-[var(--color-err)]">
                  {error}
                </div>
              )}

              <div className="mt-1 rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] p-2.5 text-[11px] leading-relaxed text-[var(--color-muted)]">
                名称、主机、端口、用户名会存到{" "}
                <span className="mono text-[var(--color-text)]">~/.RowBridge/connections.json</span>
                ；密码和配置放在同一个文件里，权限 700。
              </div>
            </div>
          </div>
        )}
      </div>

      {/* 删除确认 */}
      {confirmDelete && (
        <div className="absolute inset-0 z-40 flex items-center justify-center bg-black/50">
          <div className="w-[340px] rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] p-4">
            <div className="text-[13px] font-semibold">删除这个数据源？</div>
            <div className="mt-1.5 text-[11.5px] leading-relaxed text-[var(--color-muted)]">
              「{list.find((c) => c.id === confirmDelete)?.name}」会从配置里移除，
              配置文件里的密码也一并删除。已同步的数据不受影响。
            </div>
            <div className="mt-3 flex justify-end gap-2">
              <Button onClick={() => setConfirmDelete(null)} className="!py-1">
                取消
              </Button>
              <Button
                onClick={() => remove(confirmDelete)}
                className="!py-1 !border-[#5c2a2f] !bg-[#3d1f22] !text-[var(--color-err)]"
                testid="ds-confirm-delete"
              >
                删除
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] uppercase tracking-wide text-[var(--color-muted)]">
        {label}
      </span>
      {children}
      {hint && <span className="text-[10.5px] text-[#5c6478]">{hint}</span>}
    </label>
  );
}
