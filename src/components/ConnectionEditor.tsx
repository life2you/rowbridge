import { useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import type { ConnForm } from "../lib/types";
import { Badge, Button, Field, Spinner } from "./ui";

export interface ConnectionDraft extends ConnForm {
  /** 本地草稿 id，用于关联已保存的数据源 */
  id: string;
  label: string;
  passwordSaved: boolean;
}

export function newConnection(kind: string): ConnectionDraft {
  return {
    id: `conn_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`,
    label: kind === "src" ? "源连接" : "目标连接",
    host: "127.0.0.1",
    port: 3306,
    username: "root",
    password: "",
    connection_id: null,
    use_tls: false,
    passwordSaved: false,
  };
}

export function ConnectionEditor({
  value,
  onChange,
  title,
  accent,
  testid,
}: {
  value: ConnectionDraft;
  onChange: (v: ConnectionDraft) => void;
  title: string;
  accent: "accent" | "ok";
  testid: string;
}) {
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<
    { ok: true; version: string; schemas: number } | { ok: false; error: string } | null
  >(null);

  const set = <K extends keyof ConnectionDraft>(k: K, v: ConnectionDraft[K]) =>
    onChange({ ...value, [k]: v });

  async function handleTest() {
    setTesting(true);
    setResult(null);
    try {
      const r = await api.testConnection(value);
      setResult({ ok: true, version: r.version, schemas: r.schemas });
      // 标记密码已可用，后续请求从 connections.json 里取。
      if (value.password) {
        onChange({ ...value, passwordSaved: true });
      }
    } catch (e) {
      setResult({ ok: false, error: errorMessage(e) });
    } finally {
      setTesting(false);
    }
  }

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)] p-3.5">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span
            className="h-2 w-2 rounded-full"
            style={{
              background:
                accent === "accent" ? "var(--color-accent)" : "var(--color-ok)",
            }}
          />
          <span className="text-[13px] font-semibold">{title}</span>
        </div>
        {result?.ok && (
          <Badge tone="ok">
            v{result.version} · {result.schemas} 库
          </Badge>
        )}
        {value.passwordSaved && !result && <Badge tone="accent">已保存数据源</Badge>}
      </div>

      <div className="grid grid-cols-[1fr_84px] gap-2">
        <Field label="主机">
          <input
            data-testid={`${testid}-host`}
            value={value.host}
            onChange={(e) => set("host", e.target.value)}
            placeholder="127.0.0.1"
            spellCheck={false}
          />
        </Field>
        <Field label="端口">
          <input
            data-testid={`${testid}-port`}
            type="number"
            value={value.port}
            onChange={(e) => set("port", Number(e.target.value) || 3306)}
          />
        </Field>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <Field label="用户名">
          <input
            value={value.username}
            onChange={(e) => set("username", e.target.value)}
            spellCheck={false}
          />
        </Field>
        <Field label="密码">
          <input
            data-testid={`${testid}-password`}
            type="password"
            value={value.password ?? ""}
            onChange={(e) => set("password", e.target.value)}
            placeholder={value.passwordSaved ? "已保存，留空沿用" : ""}
          />
        </Field>
      </div>

      <div className="flex items-center gap-2">
        <Button
          onClick={handleTest}
          disabled={testing}
          className="flex-1"
          testid={`${testid}-test`}
        >
          {testing ? <Spinner /> : null}
          {testing ? "测试中…" : "测试连接"}
        </Button>
      </div>

      {result && !result.ok && (
        <div className="rounded border border-[#5c2a2f] bg-[#2a1619] px-2 py-1.5 text-[12px] text-[var(--color-err)] break-words">
          {result.error}
        </div>
      )}
    </div>
  );
}
