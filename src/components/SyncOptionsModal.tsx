import { useEffect } from "react";
import type { OptionsForm } from "../lib/types";
import { Button } from "./ui";

interface Props {
  open: boolean;
  options: OptionsForm;
  onChange: (options: OptionsForm) => void;
  onClose: () => void;
  disabled?: boolean;
}

const DEFAULT_OPTIONS: OptionsForm = {
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
  truncate_target: false,
  relax_sql_mode: false,
  continue_on_error: true,
};

export function SyncOptionsModal({
  open,
  options,
  onChange,
  onClose,
  disabled,
}: Props) {
  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [open, onClose]);

  if (!open) return null;

  const set = <K extends keyof OptionsForm>(key: K, value: OptionsForm[K]) => {
    onChange({ ...options, [key]: value });
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4">
      <div className="w-full max-w-[540px] rounded-xl border border-[var(--color-border)] bg-[var(--color-panel)] shadow-2xl flex flex-col max-h-[85vh]">
        {/* 标题栏 */}
        <div className="flex items-center justify-between border-b border-[var(--color-border)] px-4 py-3">
          <div className="flex items-center gap-2">
            <span className="text-[14px] font-semibold">⚙ 同步高级选项</span>
            <span className="text-[11px] text-[var(--color-muted)]">全局任务生效</span>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-[var(--color-muted)] hover:bg-[#232a38] hover:text-[var(--color-text)]"
          >
            ✕
          </button>
        </div>

        {/* 内容区 */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4 text-[12px]">
          {/* 1. 并发与批处理 */}
          <div>
            <div className="text-[11px] font-semibold text-[var(--color-accent)] uppercase tracking-wider mb-2">
              并发与传输批次
            </div>
            <div className="grid grid-cols-2 gap-3 bg-[#13171e] p-3 rounded-lg border border-[#202530]">
              <div>
                <label className="block text-[11px] text-[var(--color-muted)] mb-1">
                  多表并行数 (1 ~ 8)
                </label>
                <input
                  type="number"
                  min={1}
                  max={8}
                  disabled={disabled}
                  value={options.parallelism}
                  onChange={(e) => set("parallelism", Math.max(1, Math.min(8, Number(e.target.value) || 1)))}
                  className="w-full !py-1 text-[12px]"
                />
                <span className="text-[10px] text-[#5c6478] mt-0.5 block">
                  同时同步的表数量，默认 2
                </span>
              </div>
              <div>
                <label className="block text-[11px] text-[var(--color-muted)] mb-1">
                  初始批大小 (行)
                </label>
                <input
                  type="number"
                  min={100}
                  step={100}
                  disabled={disabled}
                  value={options.batch_size}
                  onChange={(e) => set("batch_size", Math.max(10, Number(e.target.value) || 100))}
                  className="w-full !py-1 text-[12px]"
                />
                <span className="text-[10px] text-[#5c6478] mt-0.5 block">
                  每批读取的起始行数，默认 1000
                </span>
              </div>
              <div>
                <label className="block text-[11px] text-[var(--color-muted)] mb-1">
                  单批最大行数上限
                </label>
                <input
                  type="number"
                  min={100}
                  step={500}
                  disabled={disabled}
                  value={options.max_batch_size}
                  onChange={(e) => set("max_batch_size", Math.max(100, Number(e.target.value) || 10000))}
                  className="w-full !py-1 text-[12px]"
                />
                <span className="text-[10px] text-[#5c6478] mt-0.5 block">
                  宽表受 65535 占位符约束
                </span>
              </div>
              <div className="flex flex-col justify-center pt-2">
                <label className="flex items-center gap-2 cursor-pointer text-[11.5px]">
                  <input
                    type="checkbox"
                    disabled={disabled}
                    checked={options.adaptive_batch}
                    onChange={(e) => set("adaptive_batch", e.target.checked)}
                    className="h-3.5 w-3.5"
                  />
                  <span>动态自适应批大小</span>
                </label>
                <span className="text-[10px] text-[#5c6478] ml-5 block mt-0.5">
                  按每批响应耗时自动放大/缩小
                </span>
              </div>
            </div>
          </div>

          {/* 2. 写入与会话选项 */}
          <div>
            <div className="text-[11px] font-semibold text-[var(--color-accent)] uppercase tracking-wider mb-2">
              写入会话与容错
            </div>
            <div className="space-y-2.5 bg-[#13171e] p-3 rounded-lg border border-[#202530]">
              <label className="flex items-start gap-2 cursor-pointer border border-[#4d2d18] bg-[#211710] p-2 rounded">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={!!options.truncate_target}
                  onChange={(e) => set("truncate_target", e.target.checked)}
                  className="h-3.5 w-3.5 mt-0.5 accent-[var(--color-warn)]"
                />
                <div>
                  <div className="text-[11.5px] font-medium text-[var(--color-warn)]">
                    同步前清空目标表 (TRUNCATE TABLE)
                  </div>
                  <div className="text-[10.5px] text-[#a88d77]">
                    在开始搬运数据前清空目标表已有数据。全量重新同步或初始化目标表时使用。
                  </div>
                </div>
              </label>

              <label className="flex items-start gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.disable_foreign_key_checks}
                  onChange={(e) => set("disable_foreign_key_checks", e.target.checked)}
                  className="h-3.5 w-3.5 mt-0.5"
                />
                <div>
                  <div className="text-[11.5px] font-medium">临时关闭外键检查 (foreign_key_checks=0)</div>
                  <div className="text-[10.5px] text-[#5c6478]">
                    大幅提速写入，且解决表依赖顺序导致的建表与写入失败风险。
                  </div>
                </div>
              </label>

              <label className="flex items-start gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.relax_sql_mode}
                  onChange={(e) => set("relax_sql_mode", e.target.checked)}
                  className="h-3.5 w-3.5 mt-0.5"
                />
                <div>
                  <div className="text-[11.5px] font-medium">宽松 SQL 模式 (sql_mode='')</div>
                  <div className="text-[10.5px] text-[#5c6478]">
                    避免目标端严格模式拦截零日期 ('0000-00-00') 或非标准历史数据。
                  </div>
                </div>
              </label>

              <label className="flex items-start gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.continue_on_error}
                  onChange={(e) => set("continue_on_error", e.target.checked)}
                  className="h-3.5 w-3.5 mt-0.5"
                />
                <div>
                  <div className="text-[11.5px] font-medium">单表同步失败后继续下一张表</div>
                  <div className="text-[10.5px] text-[#5c6478]">
                    关闭后遇任意一张表错误立即中止整个同步任务。
                  </div>
                </div>
              </label>
            </div>
          </div>

          {/* 3. 自动建表与 DDL 改写 */}
          <div>
            <div className="text-[11px] font-semibold text-[var(--color-accent)] uppercase tracking-wider mb-2">
              自动建表与 DDL 规则
            </div>
            <div className="grid grid-cols-2 gap-2.5 bg-[#13171e] p-3 rounded-lg border border-[#202530]">
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.create_database}
                  onChange={(e) => set("create_database", e.target.checked)}
                  className="h-3.5 w-3.5"
                />
                <span className="text-[11px]">目标库不存在时自动创建</span>
              </label>

              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.create_table}
                  onChange={(e) => set("create_table", e.target.checked)}
                  className="h-3.5 w-3.5"
                />
                <span className="text-[11px]">目标表不存在时按结构新建</span>
              </label>

              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.strip_foreign_keys}
                  onChange={(e) => set("strip_foreign_keys", e.target.checked)}
                  className="h-3.5 w-3.5"
                />
                <span className="text-[11px]">建表时剔除外键约束</span>
              </label>

              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.strip_auto_increment}
                  onChange={(e) => set("strip_auto_increment", e.target.checked)}
                  className="h-3.5 w-3.5"
                />
                <span className="text-[11px]">剥离 AUTO_INCREMENT 起始值</span>
              </label>

              <label className="flex items-center gap-2 cursor-pointer col-span-2">
                <input
                  type="checkbox"
                  disabled={disabled}
                  checked={options.map_collation}
                  onChange={(e) => set("map_collation", e.target.checked)}
                  className="h-3.5 w-3.5"
                />
                <span className="text-[11px]">
                  排序规则跨大版本自动降级 (如 8.0 utf8mb4_0900 → 5.7 unicode)
                </span>
              </label>
            </div>
          </div>
        </div>

        {/* 底部按钮栏 */}
        <div className="flex items-center justify-between border-t border-[var(--color-border)] px-4 py-3 bg-[var(--color-panel-2)] rounded-b-xl">
          <button
            type="button"
            disabled={disabled}
            onClick={() => onChange({ ...DEFAULT_OPTIONS, cursor_start: options.cursor_start })}
            className="text-[11px] text-[var(--color-muted)] hover:text-[var(--color-text)] underline"
          >
            恢复默认设置
          </button>
          <Button variant="primary" onClick={onClose} className="!py-1 px-4">
            确定
          </Button>
        </div>
      </div>
    </div>
  );
}
