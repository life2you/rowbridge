//! 本地 SQLite：任务历史与日志。
//!
//! 说明：**断点续跑不依赖这里**。真正的断点来自目标表的 `MAX(pk)`，
//! 这里只是为了让用户能回看「上次跑了什么、错在哪」。

use std::sync::OnceLock;

use rowbridge_core::engine::{Event, TaskReport};
use rusqlite::{params, Connection};
use serde::Serialize;

static DB_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

fn path() -> std::path::PathBuf {
    DB_PATH.get().cloned().unwrap_or_else(|| {
        std::env::temp_dir().join("rowbridge-fallback.db")
    })
}

/// 启动时调用一次。
pub fn init(app: &tauri::AppHandle) -> Result<(), String> {
    let _ = app;
    // 统一放在 ~/.RowBridge，用户好找也好备份
    let dir = crate::connections::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    let db = dir.join("rowbridge.db");
    let _ = DB_PATH.set(db.clone());

    let conn = Connection::open(&db).map_err(|e| format!("打开数据库失败: {e}"))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;

        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            src TEXT NOT NULL,
            dst TEXT NOT NULL,
            status TEXT NOT NULL,
            total_rows INTEGER NOT NULL DEFAULT 0,
            succeeded INTEGER NOT NULL DEFAULT 0,
            failed INTEGER NOT NULL DEFAULT 0,
            elapsed_ms INTEGER NOT NULL DEFAULT 0,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            error TEXT
        );

        CREATE TABLE IF NOT EXISTS task_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            ts INTEGER NOT NULL,
            level TEXT NOT NULL,
            table_name TEXT,
            message TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_logs_task ON task_logs(task_id, id);

        "#,
    )
    .map_err(|e| format!("初始化表结构失败: {e}"))?;
    Ok(())
}

fn open() -> Result<Connection, String> {
    Connection::open(path()).map_err(|e| format!("打开数据库失败: {e}"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 任务开始时写入一行 running 记录。
pub fn begin_task(
    app: &tauri::AppHandle,
    task_id: &str,
    src: &str,
    dst: &str,
) -> Result<(), String> {
    let _ = app;
    let conn = open()?;
    conn.execute(
        "INSERT OR REPLACE INTO tasks (id, src, dst, status, started_at) VALUES (?,?,?,?,?)",
        params![task_id, src, dst, "running", now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 事件落库。只记有价值的，避免把 10 万条 batch 全写进去。
pub fn record_event(app: tauri::AppHandle, task_id: &str, ev: &Event) {
    let row: Option<(&str, Option<String>, String)> = match ev {
        Event::Log {
            table,
            level,
            message,
        } => Some((level.as_str(), table.clone(), message.clone())),
        Event::TableCreated { table, .. } => Some((
            "info",
            Some(table.clone()),
            "已在目标端创建表".to_string(),
        )),
        Event::TableFailed { table, error } => Some((
            "error",
            Some(table.clone()),
            format!("同步失败: {error}"),
        )),
        Event::Warning { table, message } => {
            Some(("warn", table.clone(), message.clone()))
        }
        Event::TableDone { table, rows, elapsed_ms } => Some((
            "info",
            Some(table.clone()),
            format!("完成 {rows} 行，耗时 {elapsed_ms}ms"),
        )),
        // 每批都记会让日志爆炸，只在结束时汇总
        _ => None,
    };

    let Some((level, table, message)) = row else {
        return;
    };
    let _ = app;
    if let Ok(conn) = open() {
        let _ = conn.execute(
            "INSERT INTO task_logs (task_id, ts, level, table_name, message) VALUES (?,?,?,?,?)",
            params![task_id, now_ms(), level, table, message],
        );
    }
}

pub fn finish_task(app: tauri::AppHandle, task_id: &str, report: &TaskReport) {
    let _ = app;
    if let Ok(conn) = open() {
        let status = if report.cancelled { "cancelled" } else { "done" };
        let _ = conn.execute(
            "UPDATE tasks SET status=?, total_rows=?, succeeded=?, failed=?, elapsed_ms=?, finished_at=? WHERE id=?",
            params![
                status,
                report.total_rows as i64,
                report.succeeded as i64,
                report.failed as i64,
                report.elapsed_ms as i64,
                now_ms(),
                task_id
            ],
        );
    }
}

pub fn fail_task(app: tauri::AppHandle, task_id: &str, error: &str) {
    let _ = app;
    if let Ok(conn) = open() {
        let _ = conn.execute(
            "UPDATE tasks SET status='failed', error=?, finished_at=? WHERE id=?",
            params![error, now_ms(), task_id],
        );
    }
}

#[derive(Debug, Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub src: String,
    pub dst: String,
    pub status: String,
    pub total_rows: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub elapsed_ms: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

pub fn list_tasks(app: tauri::AppHandle) -> Result<Vec<TaskSummary>, String> {
    let _ = app;
    let conn = open()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, src, dst, status, total_rows, succeeded, failed, elapsed_ms,
                    started_at, finished_at, error
             FROM tasks ORDER BY started_at DESC LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(TaskSummary {
                id: r.get(0)?,
                src: r.get(1)?,
                dst: r.get(2)?,
                status: r.get(3)?,
                total_rows: r.get(4)?,
                succeeded: r.get(5)?,
                failed: r.get(6)?,
                elapsed_ms: r.get(7)?,
                started_at: r.get(8)?,
                finished_at: r.get(9)?,
                error: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
pub struct LogRow {
    pub ts: i64,
    pub level: String,
    pub table_name: Option<String>,
    pub message: String,
}

pub fn task_logs(
    app: tauri::AppHandle,
    task_id: &str,
    limit: usize,
) -> Result<Vec<LogRow>, String> {
    let _ = app;
    let conn = open()?;
    let mut stmt = conn
        .prepare(
            "SELECT ts, level, table_name, message FROM task_logs
             WHERE task_id = ? ORDER BY id ASC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![task_id, limit as i64], |r| {
            Ok(LogRow {
                ts: r.get(0)?,
                level: r.get(1)?,
                table_name: r.get(2)?,
                message: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}
