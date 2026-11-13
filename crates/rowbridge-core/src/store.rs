//! 本地 SQLite：任务历史与日志。
//!
//! 说明：**断点续跑不依赖这里**。真正的断点来自目标表的 `MAX(pk)`，
//! 这里只是为了让用户能回看「上次跑了什么、错在哪」。
//!
//! 存储位置与数据源配置一致：`~/.RowBridge/rowbridge.db`。
//! 与旧 Tauri 版的表结构完全兼容，老的 `tasks` / `task_logs` 记录直接沿用。

use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection};

/// 存储位置（进程内固定）。`init()` 成功后才有值。
static DB_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
/// 单连接 + 互斥锁：GPUI 主线程是唯一写者，读写都走这里。
/// 相比「每查一次开一次库」，少了频繁 open 的开销和锁竞争。
static DB_CONN: OnceLock<Mutex<Connection>> = OnceLock::new();

#[allow(dead_code)]
fn path() -> std::path::PathBuf {
    DB_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("rowbridge-fallback.db"))
}

/// 启动时调用一次。建目录、开库、建表。
pub fn init() -> Result<(), String> {
    let dir = crate::connections::config_dir()?;
    let db = dir.join("rowbridge.db");
    init_at(db)
}

/// 指定路径初始化（测试用，避免污染真实 ~/.RowBridge）。
pub fn init_at(db: std::path::PathBuf) -> Result<(), String> {
    if let Some(dir) = db.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    let _ = DB_PATH.set(db.clone());

    let conn = Connection::open(&db).map_err(|e| format!("打开数据库失败: {e}"))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA busy_timeout = 5000;

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

    let _ = DB_CONN.set(Mutex::new(conn));
    Ok(())
}

fn with_conn<T>(f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
    let conn = DB_CONN.get().ok_or_else(|| "SQLite 未初始化".to_string())?;
    let guard = conn.lock().map_err(|_| "SQLite 连接被占用".to_string())?;
    f(&guard)
}

/// 一行任务摘要。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub src: String,
    pub dst: String,
    pub status: String,
    pub total_rows: u64,
    pub succeeded: usize,
    pub failed: usize,
    pub elapsed_ms: u64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

fn upsert_task(
    conn: &Connection,
    id: &str,
    src: &str,
    dst: &str,
    status: &str,
    started_at: i64,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO tasks (id, src, dst, status, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET src=excluded.src, dst=excluded.dst",
        params![id, src, dst, status, started_at],
    )?;
    Ok(())
}

/// 任务建立时落库。应用随后被强杀，也能留下「这任务开跑了」的记录。
pub fn record_task_start(
    id: &str,
    src: &str,
    dst: &str,
    status: &str,
    started_at: i64,
) -> Result<(), String> {
    with_conn(|conn| upsert_task(conn, id, src, dst, status, started_at).map_err(|e| e.to_string()))
}

/// 任务终态落库。succeeded/failed 是**表数**，total_rows 是行数。
pub fn record_task_finish(
    id: &str,
    status: &str,
    total_rows: u64,
    succeeded: usize,
    failed: usize,
    elapsed_ms: u64,
    finished_at: i64,
    error: Option<&str>,
) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE tasks SET status=?2, total_rows=?3, succeeded=?4, failed=?5,
                    elapsed_ms=?6, finished_at=?7, error=?8
             WHERE id=?1",
            params![
                id,
                status,
                total_rows as i64,
                succeeded as i64,
                failed as i64,
                elapsed_ms as i64,
                finished_at,
                error
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 追加一条任务日志。每个日志项单独落库（低频操作）。
pub fn append_log(
    task_id: &str,
    ts: i64,
    level: &str,
    table: Option<&str>,
    message: &str,
) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO task_logs (task_id, ts, level, table_name, message)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, ts, level, table, message],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 删除任务及其日志（「删除记录」按钮）。
pub fn remove_task(id: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute("DELETE FROM task_logs WHERE task_id=?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM tasks WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// 最近的任务摘要，新在前。
pub fn recent_tasks(limit: usize) -> Result<Vec<TaskSummary>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, src, dst, status, total_rows, succeeded, failed, elapsed_ms,
                        started_at, finished_at, error
                 FROM tasks ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut rows = stmt
            .query(params![limit as i64])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            out.push(TaskSummary {
                id: row.get(0).map_err(|e| e.to_string())?,
                src: row.get(1).map_err(|e| e.to_string())?,
                dst: row.get(2).map_err(|e| e.to_string())?,
                status: row.get(3).map_err(|e| e.to_string())?,
                total_rows: row.get::<_, i64>(4).map_err(|e| e.to_string())?.max(0) as u64,
                succeeded: row.get::<_, i64>(5).map_err(|e| e.to_string())?.max(0) as usize,
                failed: row.get::<_, i64>(6).map_err(|e| e.to_string())?.max(0) as usize,
                elapsed_ms: row.get::<_, i64>(7).map_err(|e| e.to_string())?.max(0) as u64,
                started_at: row.get(8).map_err(|e| e.to_string())?,
                finished_at: row.get(9).map_err(|e| e.to_string())?,
                error: row.get(10).map_err(|e| e.to_string())?,
            });
        }
        Ok(out)
    })
}

/// 一条历史日志。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogRow {
    pub ts: i64,
    pub level: String,
    pub table_name: Option<String>,
    pub message: String,
}

/// 某任务的历史日志（旧在前）。
pub fn task_logs(task_id: &str, limit: usize) -> Result<Vec<LogRow>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT ts, level, table_name, message FROM task_logs
                 WHERE task_id = ?1 ORDER BY id ASC LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut rows = stmt
            .query(params![task_id, limit as i64])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            out.push(LogRow {
                ts: row.get(0).map_err(|e| e.to_string())?,
                level: row.get(1).map_err(|e| e.to_string())?,
                table_name: row.get(2).map_err(|e| e.to_string())?,
                message: row.get(3).map_err(|e| e.to_string())?,
            });
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_defaults_to_temp_before_init() {
        let p = path();
        assert!(
            p.to_string_lossy().contains("rowbridge-fallback"),
            "未初始化时应回退到临时目录: {p:?}"
        );
    }

    #[test]
    fn roundtrip_with_temp_db() {
        // DB_PATH/DB_CONN 是全局 OnceLock：这个测试后其它 store 测试会受影响，
        // 所以单独跑或放在最后（rustc 测试默认并行，这里用互斥锁隔离）。
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock().unwrap();

        let dir = std::env::temp_dir().join(format!("rb-store-test-{}", std::process::id()));
        let db = dir.join("t.db");
        let _ = std::fs::remove_dir_all(&dir);
        init_at(db.clone()).expect("init");

        let now = 1_700_000_000_000i64;
        record_task_start(
            "t1",
            "127.0.0.1:3306/db_a",
            "127.0.0.1:3307/db_b",
            "running",
            now,
        )
        .expect("start");
        append_log("t1", now, "info", Some("tbl"), "表 tbl 同步完成").expect("log");
        append_log("t1", now + 1, "error", None, "任务失败：x").expect("log2");
        record_task_finish("t1", "done", 42, 1, 0, 100, now + 2, None).expect("finish");

        let tasks = recent_tasks(10).expect("recent");
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.id, "t1");
        assert_eq!(t.status, "done");
        assert_eq!(t.total_rows, 42);
        assert_eq!(t.succeeded, 1);
        assert_eq!(t.failed, 0);
        assert_eq!(t.elapsed_ms, 100);
        assert_eq!(t.finished_at, Some(now + 2));

        let logs = task_logs("t1", 10).expect("logs");
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].message, "表 tbl 同步完成");
        assert_eq!(logs[1].level, "error");

        remove_task("t1").expect("remove");
        assert!(recent_tasks(10).expect("recent2").is_empty());
        assert!(task_logs("t1", 10).expect("logs2").is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
