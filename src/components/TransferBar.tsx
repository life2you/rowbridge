import type { OptionsForm } from "../lib/types";
import { formatInt } from "../lib/format";
import { Button, Spinner } from "./ui";

/**
 * 中间的传输控制条。方案 C 的核心：把「从源搬到目标」这件事摆在正中间，
 * 关键参数常驻可见，不用翻页去找。
 */
export function TransferBar({
  count,
  totalRows,
  options,
  onOptionsChange,
  onPreflight,
  onStart,
  preflighting,
  canPreflight,
  canStart,
  running,
  hasReport,
}: {
  count: number;
  totalRows: number;
  options: OptionsForm;
  onOptionsChange: (o: OptionsForm) => void;
  onPreflight: () => void;
  onStart: () => void;
  preflighting: boolean;
  canPreflight: boolean;
  canStart: boolean;
  running: boolean;
  hasReport: boolean;
}) {
  const set = <K extends keyof OptionsForm>(k: K, v: OptionsForm[K]) =>
    onOptionsChange({ ...options, [k]: v });

  return (
    <div className="flex w-[208px] shrink-0 flex-col items-center justify-center gap-3 overflow-y-auto border-x border-[var(--color-border)] bg-[var(--color-panel)] px-3 py-5">
      <div className="text-center">
        <div className="text-[26px] font-semibold leading-none tracking-tight">
          {count}
        </div>
        <div className="mt-1 text-[10.5px] text-[var(--color-muted)]">张表</div>
      </div>

      <div className="text-[20px] leading-none text-[var(--color-accent)]">↓</div>

      <div className="text-center text-[10.5px] leading-relaxed text-[var(--color-muted)]">
        {formatInt(totalRows)} 行
        <br />
        按主键分批
      </div>

      <Button
        variant="primary"
        onClick={onStart}
        disabled={!canStart}
        className="w-full"
        testid="start"
        title={!hasReport ? "请先预检" : undefined}
      >
        {running ? <Spinner /> : null}
        {running ? "同步中…" : "开始同步"}
      </Button>
      <Button
        onClick={onPreflight}
        disabled={!canPreflight}
        className="w-full"
        testid="preflight"
      >
        {preflighting && <Spinner />}
        {preflighting ? "预检中…" : "预检"}
      </Button>

      <div className="h-px w-full bg-[var(--color-border)]" />

      <div className="w-full space-y-2">
        <MiniField label="并行表数">
          <input
            type="number"
            min={1}
            max={16}
            value={options.parallelism}
            disabled={running}
            onChange={(e) => set("parallelism", Number(e.target.value) || 1)}
            className="w-full !py-[3px] !text-[11.5px]"
          />
        </MiniField>

        <MiniField label="批大小">
          <input
            type="number"
            min={1}
            value={options.batch_size}
            disabled={running}
            onChange={(e) => set("batch_size", Number(e.target.value) || 1)}
            className="w-full !py-[3px] !text-[11.5px]"
          />
        </MiniField>

        <MiniField label="起始游标">
          <select
            value={options.cursor_start}
            disabled={running}
            onChange={(e) =>
              set("cursor_start", e.target.value as OptionsForm["cursor_start"])
            }
            className="w-full !py-[3px] !text-[11.5px]"
          >
            <option value="full">全量</option>
            <option value="resume">续传</option>
            <option value="custom">自定义</option>
          </select>
        </MiniField>

        {options.cursor_start === "custom" && (
          <MiniField label="起始 id">
            <input
              value={options.start_id ?? ""}
              disabled={running}
              onChange={(e) => set("start_id", e.target.value || null)}
              placeholder="100000"
              spellCheck={false}
              className="w-full !py-[3px] !text-[11.5px]"
            />
          </MiniField>
        )}

        <MiniField label="结束 id">
          <input
            value={options.end_id ?? ""}
            disabled={running}
            onChange={(e) => set("end_id", e.target.value || null)}
            placeholder="不限"
            spellCheck={false}
            className="w-full !py-[3px] !text-[11.5px]"
          />
        </MiniField>
      </div>

      <div className="h-px w-full bg-[var(--color-border)]" />

      <div className="w-full space-y-1.5">
        <MiniCheck
          checked={options.create_table}
          onChange={(v) => set("create_table", v)}
          label="自动建表"
          hint="目标表不存在时按源结构创建"
        />
        <MiniCheck
          checked={options.strip_foreign_keys}
          onChange={(v) => set("strip_foreign_keys", v)}
          label="剥离外键"
          hint="建表时移除外键约束"
        />
        <MiniCheck
          checked={options.adaptive_batch}
          onChange={(v) => set("adaptive_batch", v)}
          label="自适应批"
          hint="根据耗时自动增减批大小"
        />
      </div>

      <details className="w-full">
        <summary className="cursor-pointer text-[10.5px] text-[var(--color-muted)] hover:text-[var(--color-text)]">
          更多选项
        </summary>
        <div className="mt-2 space-y-1.5">
          <MiniCheck
            checked={options.create_database}
            onChange={(v) => set("create_database", v)}
            label="自动建库"
          />
          <MiniCheck
            checked={options.strip_auto_increment}
            onChange={(v) => set("strip_auto_increment", v)}
            label="剥 AUTO_INCREMENT"
            hint="避免目标表主键从源表当前值起跳"
          />
          <MiniCheck
            checked={options.map_collation}
            onChange={(v) => set("map_collation", v)}
            label="collation 降级"
            hint="跨版本建表时映射 8.0 独有排序规则"
          />
          <MiniCheck
            checked={options.continue_on_error}
            onChange={(v) => set("continue_on_error", v)}
            label="单表失败继续"
          />
          <MiniCheck
            checked={options.disable_foreign_key_checks}
            onChange={(v) => set("disable_foreign_key_checks", v)}
            label="关闭外键检查"
            hint="可能写入孤儿行"
          />
          <MiniCheck
            checked={options.relax_sql_mode}
            onChange={(v) => set("relax_sql_mode", v)}
            label="放宽 sql_mode"
            hint="兼容零日期等历史数据"
          />
          <MiniField label="批大小上限">
            <input
              type="number"
              min={1}
              value={options.max_batch_size}
              disabled={running}
              onChange={(e) => set("max_batch_size", Number(e.target.value) || 1)}
              className="w-full !py-[3px] !text-[11.5px]"
            />
          </MiniField>
        </div>
      </details>
    </div>
  );
}

function MiniField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <label className="mb-0.5 block text-[10px] uppercase tracking-wide text-[var(--color-muted)]">
        {label}
      </label>
      {children}
    </div>
  );
}

function MiniCheck({
  checked,
  onChange,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  hint?: string;
}) {
  return (
    <label
      className="flex cursor-pointer items-center gap-1.5 text-[10.5px] text-[var(--color-text)]"
      title={hint}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="h-3 w-3 shrink-0 accent-[var(--color-accent)]"
      />
      <span className="truncate">{label}</span>
    </label>
  );
}