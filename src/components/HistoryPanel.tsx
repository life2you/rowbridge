import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import type { LogRow, TaskSummary } from "../lib/types";
import { formatDuration, formatInt, formatTime, cn } from "../lib/format";
import { Badge, Empty, Spinner } from "./ui";

const STATUS_TONE: Record<string, "ok" | "err" | "warn" | "muted"> = {
  done: "ok",
  failed: "err",
  cancelled: "warn",
  running: "warn",
};

export function HistoryPanel() {
  const [tasks, setTasks] = useState<TaskSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [logs, setLogs] = useState<LogRow[]>([]);

  useEffect(() => {
    let cancelled = false;
    api
      .listTasks()
      .then((t) => {
        if (!cancelled) setTasks(t);
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function openLogs(id: string) {
    if (active === id) {
      setActive(null);
      return;
    }
    setActive(id);
    setLogs([]);
    try {
      setLogs(await api.taskLogs(id, 500));
    } catch (e) {
      setLogs([
        {
          ts: Date.now(),
          level: "error",
          table_name: null,
          message: errorMessage(e),
        },
      ]);
    }
  }

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center text-[var(--color-muted)]">
        <Spinner /> <span className="ml-2">加载中…</span>
      </div>
    );
  }
  if (error) {
    return <Empty>读取历史失败：{error}</Empty>;
  }
  if (tasks.length === 0) {
    return <Empty>还没有同步记录。完成一次同步后，这里会保留任务摘要与日志。</Empty>;
  }

  return (
    <div className="h-full overflow-y-auto p-3">
      <div className="space-y-1.5">
        {tasks.map((t) => (
          <div
            key={t.id}
            className="rounded-lg border border-[var(--color-border)] bg-[var(--color-panel)]"
          >
            <button
              type="button"
              onClick={() => openLogs(t.id)}
              className="flex w-full flex-wrap items-center gap-2 px-3 py-2 text-left hover:bg-[#1a1e27]"
            >
              <Badge tone={STATUS_TONE[t.status] ?? "muted"}>{t.status}</Badge>
              <span className="text-[12px] text-[var(--color-muted)]">
                {formatTime(t.started_at)}
              </span>
              <span className="text-[12.5px]">
                {formatInt(t.total_rows)} 行
              </span>
              <span className="text-[11.5px] text-[var(--color-muted)]">
                成功 {t.succeeded} · 失败 {t.failed} · {formatDuration(t.elapsed_ms)}
              </span>
              {t.error && (
                <span className="max-w-[380px] truncate text-[11px] text-[var(--color-err)]">
                  {t.error}
                </span>
              )}
              <span className="ml-auto text-[11px] text-[var(--color-muted)]">
                {active === t.id ? "收起" : "日志"}
              </span>
            </button>

            {active === t.id && (
              <div className="max-h-64 overflow-y-auto border-t border-[var(--color-border)] bg-[#0c0e12] px-3 py-2 mono text-[11.5px] leading-relaxed">
                {logs.length === 0 && (
                  <div className="text-[#5c6478]">无日志</div>
                )}
                {logs.map((l, i) => (
                  <div key={i} className="flex gap-2">
                    <span className="shrink-0 text-[#5c6478]">
                      {formatTime(l.ts).slice(11)}
                    </span>
                    {l.table_name && (
                      <span className="shrink-0 text-[var(--color-accent)]">
                        {l.table_name}
                      </span>
                    )}
                    <span
                      className={cn(
                        "break-all",
                        l.level === "error"
                          ? "text-[var(--color-err)]"
                          : l.level === "warn"
                            ? "text-[var(--color-warn)]"
                            : "text-[#a9b4c6]",
                      )}
                    >
                      {l.message}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
