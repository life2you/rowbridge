/** 与 Rust 侧 commands.rs 对应的类型。 */

export type Severity = "error" | "warning" | "info" | "ok";

export interface Check {
  severity: Severity;
  code: string;
  message: string;
}

export interface RewriteChange {
  kind: string;
  detail: string;
}

export interface RewriteResult {
  ddl: string;
  changes: RewriteChange[];
  warnings: string[];
}

export interface TablePreflight {
  source_schema: string;
  table: string;
  target_schema: string;
  target_table: string;
  syncable: boolean;
  primary_key: string | null;
  primary_key_unsigned: boolean;
  columns: string[];
  checks: Check[];
  needs_create_table: boolean;
  create_table: RewriteResult | null;
  source_max_pk: string | null;
  target_max_pk: string | null;
  source_rows_estimate: number;
  target_rows_exists: boolean;
  cursor_start: CursorMode;
  start_id: string | null;
  end_id: string | null;
}

export interface PreflightReport {
  tables: TablePreflight[];
  source_version: string;
  target_version: string;
  same_instance: boolean;
  checks: Check[];
}

export interface SchemaInfo {
  name: string;
  charset: string | null;
  collation: string | null;
}

export interface TableRow {
  name: string;
  rows_estimate: number;
  engine: string | null;
  primary_key: string | null;
  unsupported_reason: string | null;
  is_view: boolean;
  /** 主键最大值，逐表游标做区间行数估算用 */
  max_pk: number | null;
}

export interface ColumnInfo {
  name: string;
  data_type: string;
  column_type: string;
  is_nullable: boolean;
  column_default: string | null;
  extra: string;
  charset: string | null;
  collation: string | null;
  is_primary: boolean;
}

/** 勾选表时异步校验的结果 */
export interface ValidatedTable {
  name: string;
  primary_key: string | null;
  primary_key_unsigned: boolean;
  unsupported_reason: string | null;
  max_pk: number | null;
  trigger_count: number;
}

export interface TableDetail {
  schema: string;
  table: string;
  columns: ColumnInfo[];
  primary_key: string[];
  triggers: string[];
}

export interface ConnForm {
  host: string;
  port: number;
  username: string;
  password?: string | null;
  connection_id?: string | null;
  use_tls?: boolean;
}

/** 已保存的数据源 */
export interface SavedConnection {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  /** 明文保存在 ~/.RowBridge/connections.json（用户要求，不再用钥匙串） */
  password: string;
  use_tls: boolean;
  last_schema: string | null;
}

export interface TestResult {
  version: string;
  is_mariadb: boolean;
  schemas: number;
}

export interface OptionsForm {
  parallelism: number;
  batch_size: number;
  max_batch_size: number;
  adaptive_batch: boolean;
  cursor_start: "full" | "resume" | "custom";
  start_id?: string | null;
  end_id?: string | null;
  create_table: boolean;
  create_database: boolean;
  strip_foreign_keys: boolean;
  strip_auto_increment: boolean;
  map_collation: boolean;
  disable_foreign_key_checks: boolean;
  relax_sql_mode: boolean;
  continue_on_error: boolean;
}

export type CursorMode = "full" | "resume" | "custom";

export interface TableSpec {
  table: string;
  dst_table?: string | null;
  /** 表级游标；不传则跟随全局 */
  cursor_start?: CursorMode | null;
  start_id?: string | null;
  end_id?: string | null;
}

/** 每张表的游标覆写（空字符串表示「不限」） */
export interface CursorOverride {
  cursor_start?: CursorMode | null;
  start_id?: string | null;
  end_id?: string | null;
}

export interface RunRequest {
  source: ConnForm;
  target: ConnForm;
  src_schema: string;
  dst_schema: string;
  tables: TableSpec[];
  options: OptionsForm;
}

export type SyncEvent =
  | { type: "task_started"; tables: number }
  | { type: "table_started"; table: string; columns: number; needs_create_table: boolean }
  | { type: "table_created"; table: string; ddl: string }
  | {
      type: "batch_done";
      table: string;
      rows: number;
      total_rows: number;
      cursor: string;
      rows_per_sec: number;
      batch_size: number;
      elapsed_ms: number;
    }
  | { type: "table_done"; table: string; rows: number; elapsed_ms: number }
  | { type: "table_failed"; table: string; error: string }
  | { type: "warning"; table: string | null; message: string }
  | { type: "log"; table: string | null; level: string; message: string }
  | { type: "task_done"; total_rows: number; succeeded: number; failed: number; elapsed_ms: number }
  | { type: "cancelled" };

export interface TableStats {
  table: string;
  rows: number;
  batches: number;
  cursor: string;
  elapsed_ms: number;
  status: "running" | "done" | "failed" | "cancelled";
  error: string | null;
}

export interface TaskReport {
  tables: TableStats[];
  total_rows: number;
  succeeded: number;
  failed: number;
  /** 因取消停在半路的表：既没跑完也没报错 */
  partial: number;
  elapsed_ms: number;
  cancelled: boolean;
}

export interface TaskSummary {
  id: string;
  src: string;
  dst: string;
  status: string;
  total_rows: number;
  succeeded: number;
  failed: number;
  elapsed_ms: number;
  started_at: number;
  finished_at: number | null;
  error: string | null;
}

export interface LogRow {
  ts: number;
  level: string;
  table_name: string | null;
  message: string;
}

/** 一张表在整个 UI 流程中的状态。 */
export interface TableSelection {
  name: string;
  rows_estimate: number;
  primary_key: string | null;
  unsupported_reason: string | null;
  selected: boolean;
}
