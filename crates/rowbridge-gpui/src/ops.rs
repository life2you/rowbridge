//! 数据库操作桥接层。
//!
//! GPUI 运行在主线程，MySQL 查询必须放到 Tokio 运行时里跑：
//! 这里提供统一的 `spawn` 包装，把结果通过 oneshot 送回 UI 任务。

use std::future::Future;
use std::sync::OnceLock;

use rowbridge_core::config::{ConnConfig, TaskConfig};
use rowbridge_core::engine::{self, Event, EventSink, SchemaSyncReport, TaskReport};
use rowbridge_core::introspect::{self, SchemaInfo};
use rowbridge_core::preflight::{self, PreflightReport};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// 全局唯一的 Tokio 运行时（所有 MySQL IO 都在这里）。
pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .thread_name("rowbridge-io")
            .build()
            .expect("创建 Tokio 运行时失败")
    })
}

/// 在 Tokio 运行时里执行一个 Future，UI 侧 await 接收结果。
pub fn spawn<T, F>(fut: F) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    rt().spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx
}

pub async fn connect(cfg: &ConnConfig) -> Result<mysql_async::Conn, String> {
    let opts = cfg.to_url().map_err(|e| e.to_string())?;
    mysql_async::Conn::new(opts)
        .await
        .map_err(|e| format!("连接失败: {e}"))
}

pub async fn test_connection(cfg: ConnConfig) -> Result<String, String> {
    let mut conn = connect(&cfg).await?;
    let v = introspect::server_version(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    let schemas = introspect::list_schemas(&mut conn)
        .await
        .map_err(|e| e.to_string())?
        .len();
    let _ = conn.disconnect().await;
    Ok(format!("MySQL {}.{}.{} · {schemas} 个库", v.0, v.1, v.2))
}

pub async fn list_schemas(cfg: ConnConfig) -> Result<Vec<SchemaInfo>, String> {
    let mut conn = connect(&cfg).await?;
    let out = introspect::list_schemas(&mut conn)
        .await
        .map_err(|e| e.to_string());
    let _ = conn.disconnect().await;
    out
}

/// 表清单里一行（快路径，不含列 / MAX(pk)）。
#[derive(Debug, Clone)]
pub struct TableMeta {
    pub name: String,
    pub rows_estimate: u64,
    pub primary_key: Option<String>,
    pub unsupported_reason: Option<String>,
}

pub async fn list_tables(cfg: ConnConfig, schema: String) -> Result<Vec<TableMeta>, String> {
    let mut conn = connect(&cfg).await?;
    let out = list_tables_inner(&mut conn, &schema).await;
    let _ = conn.disconnect().await;
    out
}

async fn list_tables_inner(
    conn: &mut mysql_async::Conn,
    schema: &str,
) -> Result<Vec<TableMeta>, String> {
    let tables = introspect::list_tables(conn, schema)
        .await
        .map_err(|e| e.to_string())?;
    let pk_rows = introspect::list_primary_keys(conn, schema)
        .await
        .map_err(|e| e.to_string())?;

    let mut pk_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (table, col, _seq, _non_unique) in pk_rows {
        pk_map.entry(table).or_default().push(col);
    }

    Ok(tables
        .into_iter()
        .map(|t| {
            let is_view = t
                .table_type
                .as_deref()
                .map(|ty| ty.eq_ignore_ascii_case("VIEW"))
                .unwrap_or(false);
            let (primary_key, unsupported_reason) = if is_view {
                (None, Some("视图".to_string()))
            } else {
                match pk_map.get(&t.name) {
                    Some(cols) if cols.len() > 1 => {
                        (None, Some(format!("复合主键 ({})", cols.join(", "))))
                    }
                    Some(cols) if cols.len() == 1 => (Some(cols[0].clone()), None),
                    _ => (None, Some("无主键".to_string())),
                }
            };
            TableMeta {
                name: t.name,
                rows_estimate: t.rows_estimate.unwrap_or(0),
                primary_key,
                unsupported_reason,
            }
        })
        .collect())
}

pub async fn run_preflight(cfg: TaskConfig) -> Result<PreflightReport, String> {
    let same = preflight::same_instance(&cfg.source, &cfg.target);
    let mut sc = connect(&cfg.source)
        .await
        .map_err(|e| format!("源库: {e}"))?;
    let mut dc = connect(&cfg.target)
        .await
        .map_err(|e| format!("目标库: {e}"))?;
    let report = preflight::run(&mut sc, &mut dc, &cfg.tables, &cfg.options, same)
        .await
        .map_err(|e| e.to_string());
    let _ = sc.disconnect().await;
    let _ = dc.disconnect().await;
    report
}

pub async fn run_sync_schema(
    cfg: TaskConfig,
    report: PreflightReport,
) -> Result<SchemaSyncReport, String> {
    engine::sync_schema(&cfg, &report)
        .await
        .map_err(|e| e.to_string())
}

/// 一次同步任务的句柄。
pub struct RunHandle {
    /// 引擎事件流
    pub events: mpsc::UnboundedReceiver<Event>,
    /// 取消令牌
    pub cancel: CancellationToken,
    /// 任务最终结果（事件流结束后才就绪）
    pub done: oneshot::Receiver<Result<TaskReport, String>>,
}

pub fn start_sync(cfg: TaskConfig, report: PreflightReport) -> RunHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let sink = EventSink::new(tx);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let (dtx, drx) = oneshot::channel();

    rt().spawn(async move {
        let result = engine::run_task(cfg, &report, sink, token)
            .await
            .map_err(|e| e.to_string());
        let _ = dtx.send(result);
    });

    RunHandle {
        events: rx,
        cancel,
        done: drx,
    }
}
