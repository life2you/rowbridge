import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  ConnForm,
  LogRow,
  PreflightReport,
  RunRequest,
  SchemaInfo,
  SyncEvent,
  TableDetail,
  TableRow,
  TaskReport,
  SavedConnection,
  TaskSummary,
  TestResult,
} from "./types";

export const api = {
  testConnection: (form: ConnForm) =>
    invoke<TestResult>("test_connection", { form }),

  listSchemas: (form: ConnForm) =>
    invoke<SchemaInfo[]>("list_schemas", { form }),

  listTables: (form: ConnForm, schema: string) =>
    invoke<TableRow[]>("list_tables", { form, schema }),

  describeTable: (form: ConnForm, schema: string, table: string) =>
    invoke<TableDetail>("describe_table", { form, schema, table }),

  preflight: (req: RunRequest) =>
    invoke<PreflightReport>("preflight", { req }),

  /** 返回值在任务结束时才 resolve；过程中通过 onEvent 推送进度。 */
  startSync: (req: RunRequest, taskId: string, report: PreflightReport, onEvent: (e: SyncEvent) => void) => {
    const channel = new Channel<SyncEvent>();
    channel.onmessage = onEvent;
    return invoke<TaskReport>("start_sync", {
      req,
      taskId,
      report,
      onEvent: channel,
    });
  },

  cancelSync: (taskId: string) => invoke<boolean>("cancel_sync", { taskId }),

  listConnections: () => invoke<SavedConnection[]>("list_connections"),

  saveConnection: (conn: SavedConnection) =>
    invoke<SavedConnection[]>("save_connection", { conn }),

  removeConnection: (connectionId: string) =>
    invoke<SavedConnection[]>("remove_connection", { connectionId }),

  rememberSchema: (connectionId: string, schema: string) =>
    invoke<void>("remember_schema", { connectionId, schema }),

  connectionsPath: () => invoke<string>("connections_path"),

  revealConfigDir: () => invoke<void>("reveal_config_dir"),

  listTasks: () => invoke<TaskSummary[]>("list_tasks"),

  taskLogs: (taskId: string, limit = 500) =>
    invoke<LogRow[]>("task_logs", { taskId, limit }),
};

export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
