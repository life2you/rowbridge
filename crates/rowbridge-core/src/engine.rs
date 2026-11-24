//! 同步引擎。
//!
//! 主循环的核心不变式（见 `docs/DESIGN.md` §3）：
//!
//! ```sql
//! SELECT cols FROM src WHERE pk > ? ORDER BY pk ASC LIMIT n
//! ```
//!
//! **`ORDER BY pk` 不是优化，是正确性前提**：游标取的是「本批最后一行」的 id，
//! 若返回顺序不按 pk 递增，最后一行就不是本批最大值，中间的行会被静默跳过。
//!
//! 写入端用 `INSERT ... ON DUPLICATE KEY UPDATE`，因此任务可随时中断重跑，
//! 断点由目标表 `MAX(pk)` 自然推导，不需要额外的进度表。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::stream::{FuturesUnordered, StreamExt};
use mysql_async::prelude::*;
use mysql_async::{Conn, Params, Value};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::{ConnConfig, CursorStart, TableTaskConfig, TaskConfig, TaskOptions, WriteMode};
use crate::introspect;
use crate::preflight::{self, TablePreflight};
use crate::value::{
    self, i128_to_pk_value, parse_cursor_value, qualified, quote_ident, value_as_i128, Cursor,
};
use crate::{Error, Result};

/// MySQL 单语句占位符上限。
const MAX_PLACEHOLDERS: usize = 65_535;
/// 包大小预算占 `max_allowed_packet` 的比例。
const PACKET_BUDGET_RATIO: f64 = 0.6;
/// 自适应批大小的下限。
const MIN_BATCH: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    TaskStarted {
        tables: usize,
    },
    TableStarted {
        table: String,
        columns: usize,
        needs_create_table: bool,
    },
    TableCreated {
        table: String,
        ddl: String,
    },
    BatchDone {
        table: String,
        rows: u64,
        total_rows: u64,
        cursor: String,
        rows_per_sec: f64,
        batch_size: usize,
        elapsed_ms: u64,
    },
    /// 单表启用多分片并行时，下发各分片规划清单
    TableShardsPlanned {
        table: String,
        shards: Vec<ShardInfo>,
    },
    /// 某个分片完成了一批同步
    ShardBatchDone {
        table: String,
        shard_index: usize,
        rows: u64,
        total_rows: u64,
        cursor: String,
        rows_per_sec: f64,
        percent: f32,
    },
    /// 某个分片同步完成
    ShardDone {
        table: String,
        shard_index: usize,
        rows: u64,
        elapsed_ms: u64,
    },
    /// 某个分片同步失败
    ShardFailed {
        table: String,
        shard_index: usize,
        error: String,
    },
    TableDone {
        table: String,
        rows: u64,
        elapsed_ms: u64,
    },
    TableFailed {
        table: String,
        error: String,
    },
    Warning {
        table: Option<String>,
        message: String,
    },
    Log {
        table: Option<String>,
        level: String,
        message: String,
    },
    TaskDone {
        total_rows: u64,
        succeeded: usize,
        failed: usize,
        elapsed_ms: u64,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShardInfo {
    pub index: usize,
    pub total: usize,
    pub range: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableStats {
    pub table: String,
    pub rows: u64,
    pub batches: u64,
    pub cursor: String,
    pub elapsed_ms: u64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchemaSyncReport {
    pub databases_created: Vec<String>,
    pub tables_created: Vec<String>,
    pub columns_added: Vec<String>,
    pub errors: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskReport {
    pub tables: Vec<TableStats>,
    pub total_rows: u64,
    pub succeeded: usize,
    pub failed: usize,
    /// 因取消而停在半路的表：既没跑完，也没报错。
    /// 单独计数，否则这类表在 succeeded + failed 的账面上会凭空消失。
    pub partial: usize,
    pub elapsed_ms: u64,
    pub cancelled: bool,
}

/// 事件发送器。允许没有监听者（CLI 场景）。
#[derive(Clone)]
pub struct EventSink {
    tx: Option<mpsc::UnboundedSender<Event>>,
}

impl EventSink {
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        Self { tx: Some(tx) }
    }
    pub fn silent() -> Self {
        Self { tx: None }
    }
    pub fn emit(&self, e: Event) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(e);
        }
    }
}

/// 单表的运行态。
struct TableRun {
    pk: String,
    pk_index: usize,
    columns: Vec<String>,
    cursor: Cursor,
    /// 是否包含当前游标值（第一批自定义起始 ID 为 true，即闭区间 `>=`；后续批次为 false，即 `>`）
    cursor_inclusive: bool,
    end_id: Option<i128>,
    /// 结束主键的原始 Value，用于下推到 SQL 的 `pk <= ?`
    end_pk: Option<Value>,
    batch_size: usize,
    src_schema: String,
    dst_schema: String,
    src_table: String,
    dst_table: String,
    filter: Option<String>,
}

/// 仅同步表结构（包括自动建库、创建缺失表、为已有表追加缺失列）。
/// 纯 DDL 操作，不读写任何数据行。
pub async fn sync_schema(
    cfg: &TaskConfig,
    preflight: &preflight::PreflightReport,
) -> Result<SchemaSyncReport> {
    let started = Instant::now();
    let mut out = SchemaSyncReport::default();

    let mut dst = connect(&cfg.target, "目标").await?;
    let mut src = connect(&cfg.source, "源").await?;

    let mut created_dbs = std::collections::HashSet::new();

    for pf in &preflight.tables {
        // 1. 如果需要建库
        if !created_dbs.contains(&pf.target_schema) {
            let db_exists = introspect::schema_exists(&mut dst, &pf.target_schema).await?;
            if !db_exists && cfg.options.create_database {
                let (charset, collation) = source_schema_charset(&mut src, &pf.source_schema)
                    .await
                    .unwrap_or(("utf8mb4".to_string(), "utf8mb4_general_ci".to_string()));
                let sql = format!(
                    "CREATE DATABASE IF NOT EXISTS {} DEFAULT CHARACTER SET {charset} COLLATE {collation}",
                    crate::value::quote_ident(&pf.target_schema)
                );
                dst.query_drop(&sql).await.map_err(|e| {
                    Error::Other(format!("创建目标库 {} 失败: {e}", pf.target_schema))
                })?;
                out.databases_created.push(pf.target_schema.clone());
            }
            created_dbs.insert(pf.target_schema.clone());
        }

        // 2. 如果需要建表
        if pf.needs_create_table {
            if let Some(ct) = &pf.create_table {
                match dst.query_drop(&ct.ddl).await {
                    Ok(()) => {
                        out.tables_created
                            .push(format!("{}.{}", pf.target_schema, pf.target_table));
                    }
                    Err(e) => {
                        let err_msg =
                            format!("创建表 {}.{} 失败: {e}", pf.target_schema, pf.target_table);
                        out.errors.push(err_msg.clone());
                        if !cfg.options.continue_on_error {
                            return Err(Error::Other(err_msg));
                        }
                    }
                }
            }
        }

        // 3. 如果已有表缺少列（结构变更）：按实时结构补，不用预检快照
        if !pf.needs_create_table {
            match ensure_columns(&mut src, &mut dst, pf, &cfg.options, &EventSink::silent()).await {
                Ok(done) => out.columns_added.extend(done),
                Err(e) => {
                    let err_msg = e.to_string();
                    out.errors.push(err_msg.clone());
                    if !cfg.options.continue_on_error {
                        return Err(Error::Other(err_msg));
                    }
                }
            }
        }
    }

    let _ = src.disconnect().await;
    let _ = dst.disconnect().await;

    out.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(out)
}

/// 任务暂停与恢复控制器。
#[derive(Clone, Default)]
pub struct PauseController {
    paused: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl PauseController {
    pub fn new() -> Self {
        Self::default()
    }

    /// 触发暂停
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// 恢复继续
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 当前是否处于暂停状态
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 若处于暂停状态，则异步等待直到被恢复或取消
    pub async fn check_pause(&self, cancel: &CancellationToken) {
        while self.is_paused() && !cancel.is_cancelled() {
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = cancel.cancelled() => break,
            }
        }
    }
}

/// 执行整个同步任务（多表并行）。
pub async fn run_task(
    cfg: TaskConfig,
    preflight: &preflight::PreflightReport,
    sink: EventSink,
    cancel: CancellationToken,
    pause: PauseController,
) -> Result<TaskReport> {
    let started = Instant::now();
    let opts = cfg.options.clone();
    let parallelism = opts.parallelism.max(1);

    let mut runnable: Vec<TablePreflight> =
        preflight.syncable_tables().into_iter().cloned().collect();
    if runnable.is_empty() {
        return Err(Error::Preflight("没有可同步的表（预检全部未通过）".into()));
    }
    runnable.sort_by(|a, b| a.table.cmp(&b.table));

    sink.emit(Event::TaskStarted {
        tables: runnable.len(),
    });

    // 建库/建表串行执行，避免并发 DDL 冲突。
    let mut dst = connect(&cfg.target, "目标").await?;
    let mut src = connect(&cfg.source, "源").await?;
    let mut ready: Vec<TablePreflight> = Vec::new();
    let mut report = TaskReport::default();

    for pf in runnable {
        if cancel.is_cancelled() {
            break;
        }
        pause.check_pause(&cancel).await;
        if cancel.is_cancelled() {
            break;
        }
        let tbl_cfg = cfg.tables.iter().find(|t| t.table == pf.table);
        let should_truncate = tbl_cfg
            .map(|t| t.effective_truncate_target(&opts))
            .unwrap_or(opts.truncate_target);
        let should_create = tbl_cfg
            .map(|t| t.effective_create_table(&opts))
            .unwrap_or(opts.create_table);
        let should_continue = tbl_cfg
            .map(|t| t.effective_continue_on_error(&opts))
            .unwrap_or(opts.continue_on_error);

        match prepare_table(
            &mut src,
            &mut dst,
            &pf,
            &opts,
            should_truncate,
            should_create,
            &sink,
        )
        .await
        {
            Ok(()) => ready.push(pf),
            Err(e) => {
                sink.emit(Event::TableFailed {
                    table: pf.table.clone(),
                    error: e.to_string(),
                });
                report.failed += 1;
                report.tables.push(TableStats {
                    table: pf.table.clone(),
                    status: "failed".into(),
                    error: Some(e.to_string()),
                    ..Default::default()
                });
                if !should_continue {
                    drop(src);
                    drop(dst);
                    return Err(e);
                }
            }
        }
    }
    drop(src);
    drop(dst);

    let sem = Arc::new(tokio::sync::Semaphore::new(parallelism));
    let mut futs = FuturesUnordered::new();

    for pf in ready {
        let permit = match sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => break,
        };
        let src_cfg = cfg.source.clone();
        let dst_cfg = cfg.target.clone();
        let opts = opts.clone();
        let sink = sink.clone();
        let cancel = cancel.clone();
        let pause = pause.clone();
        // 按表名找回它自己的配置；找不到就用手上这份
        let table_cfg = cfg
            .tables
            .iter()
            .find(|t| t.table == pf.table)
            .cloned()
            .unwrap_or_else(|| TableTaskConfig {
                table: pf.table.clone(),
                src_schema: pf.source_schema.clone(),
                dst_schema: pf.target_schema.clone(),
                dst_table: Some(pf.target_table.clone()),
                ..Default::default()
            });

        futs.push(tokio::spawn(async move {
            let _permit = permit;
            run_table(src_cfg, dst_cfg, opts, table_cfg, pf, sink, cancel, pause).await
        }));
    }

    let mut cancelled_mid = false;

    while let Some(joined) = futs.next().await {
        match joined {
            Ok(Ok(stats)) => {
                report.total_rows += stats.rows;
                match stats.status.as_str() {
                    "done" => report.succeeded += 1,
                    "cancelled" => {
                        cancelled_mid = true;
                        report.partial += 1;
                    }
                    _ => report.failed += 1,
                }
                report.tables.push(stats);
            }
            Ok(Err(e)) => {
                report.failed += 1;
                sink.emit(Event::Log {
                    table: None,
                    level: "error".into(),
                    message: e.to_string(),
                });
            }
            Err(join_err) => {
                report.failed += 1;
                sink.emit(Event::Log {
                    table: None,
                    level: "error".into(),
                    message: format!("工作线程异常: {join_err}"),
                });
            }
        }
    }

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    report.cancelled = cancel.is_cancelled() || cancelled_mid;

    if report.cancelled {
        sink.emit(Event::Cancelled);
    } else {
        sink.emit(Event::TaskDone {
            total_rows: report.total_rows,
            succeeded: report.succeeded,
            failed: report.failed,
            elapsed_ms: report.elapsed_ms,
        });
    }
    Ok(report)
}

async fn connect(cfg: &ConnConfig, label: &str) -> Result<Conn> {
    Conn::new(cfg.to_url()?)
        .await
        .map_err(|e| Error::Connect(format!("连接{label}库失败: {e}")))
}

/// 建库 + 建表。
/// 目标表缺列时自动补列。
///
/// 不直接用预检里算好的 `alter_sqls`：预检到真正开始同步之间目标表结构可能被改过，
/// 用实时 introspection 再算一遍，才不会「预检说没事、同步报 1054」。
async fn ensure_columns(
    src: &mut Conn,
    dst: &mut Conn,
    pf: &TablePreflight,
    opts: &TaskOptions,
    sink: &EventSink,
) -> Result<Vec<String>> {
    let src_detail = introspect::table_detail(src, &pf.source_schema, &pf.table).await?;
    let dst_detail = introspect::table_detail(dst, &pf.target_schema, &pf.target_table).await?;

    let mut missing: Vec<String> = Vec::new();
    let mut missing_in_source: Vec<String> = Vec::new();
    let mut ddls: Vec<String> = Vec::new();
    let ver = introspect::server_version(dst).await.unwrap_or((0, 0, 0));
    // 目标列顺序的快照；每追加一列就同步更新，后面的列才能接在它后面，
    // 否则多条 ADD COLUMN 都以同一列为锚点，顺序会颠倒。
    let mut dst_cols: Vec<String> = dst_detail.columns.iter().map(|c| c.name.clone()).collect();
    for name in &pf.columns {
        let Some(s_col) = src_detail.columns.iter().find(|c| &c.name == name) else {
            missing_in_source.push(name.clone());
            continue;
        };
        if dst_cols.iter().any(|c| c == name) {
            continue;
        }
        let src_idx = src_detail
            .columns
            .iter()
            .position(|c| &c.name == name)
            .unwrap_or(0);
        // 源表里紧邻的前一个「目标端已存在」的列，作为 AFTER 锚点
        let after_col: Option<String> = if src_idx > 0 {
            src_detail.columns[..src_idx]
                .iter()
                .rev()
                .find(|c| dst_cols.contains(&c.name))
                .map(|c| c.name.clone())
        } else {
            None
        };
        let insert_at = after_col
            .as_ref()
            .and_then(|a| dst_cols.iter().position(|c| c == a))
            .map(|p| p + 1)
            .unwrap_or(0);
        dst_cols.insert(insert_at, name.clone());
        ddls.push(preflight::build_add_column_ddl(
            &pf.target_schema,
            &pf.target_table,
            s_col,
            after_col.as_deref(),
            ver,
        ));
        missing.push(name.clone());
    }

    // 源表里都找不到这些列，说明预检结果过期（源表结构变了），SELECT 必然报 1054。
    if !missing_in_source.is_empty() {
        return Err(Error::Other(format!(
            "源表 {}.{} 已不存在这些列：{missing_in_source:?}（预检结果已过期，请重新预检）",
            pf.source_schema, pf.table
        )));
    }

    if ddls.is_empty() {
        return Ok(Vec::new());
    }

    if !opts.add_missing_columns {
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "warn".into(),
            message: format!("目标表缺少列 {missing:?} 且未开启「缺列自动补」，写入可能报 1054"),
        });
        return Ok(Vec::new());
    }

    let mut done: Vec<String> = Vec::new();
    for alter_sql in ddls {
        match dst.query_drop(&alter_sql).await {
            Ok(()) => {
                done.push(alter_sql.clone());
                sink.emit(Event::Log {
                    table: Some(pf.table.clone()),
                    level: "info".into(),
                    message: format!("目标表缺列，已自动补列：{alter_sql}"),
                })
            }
            Err(e) => {
                let msg = e.to_string();
                // 并发同步时列可能刚被另一个任务补上，重复列不算失败。
                if msg.contains("Duplicate column") {
                    continue;
                }
                return Err(Error::Other(format!(
                    "自动补列失败: {msg}\nSQL: {alter_sql}"
                )));
            }
        }
    }
    if !missing.is_empty() {
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!("已补齐 {} 个缺失列：{missing:?}", missing.len()),
        });
    }
    Ok(done)
}

async fn prepare_table(
    src: &mut Conn,
    dst: &mut Conn,
    pf: &TablePreflight,
    opts: &TaskOptions,
    should_truncate: bool,
    should_create: bool,
    sink: &EventSink,
) -> Result<()> {
    if pf.has_blocking_error() {
        let detail = pf
            .checks
            .iter()
            .filter(|c| c.severity == preflight::Severity::Error)
            .map(|c| c.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::Preflight(format!("{}: {detail}", pf.table)));
    }

    if !introspect::schema_exists(dst, &pf.target_schema).await? {
        if !opts.create_database {
            return Err(Error::Other(format!(
                "目标库 {} 不存在，且未开启自动建库",
                pf.target_schema
            )));
        }
        // 沿用源库的字符集，从源库连接查询，避免跨实例时因目标端无同名库而查错。
        let (charset, collation) = source_schema_charset(src, &pf.source_schema)
            .await
            .unwrap_or(("utf8mb4".to_string(), "utf8mb4_general_ci".to_string()));
        let sql = format!(
            "CREATE DATABASE IF NOT EXISTS {} DEFAULT CHARACTER SET {} COLLATE {}",
            quote_ident(&pf.target_schema),
            charset,
            collation
        );
        dst.query_drop(sql).await?;
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!("已创建目标库 {} ({charset}/{collation})", pf.target_schema),
        });
    }

    if introspect::table_exists(dst, &pf.target_schema, &pf.target_table).await? {
        // 目标表缺列：写入会直接报 1054 Unknown column，所以先把结构补齐。
        // 补列放在 TRUNCATE 之前 —— 万一补列失败，至少不会把已有数据清掉。
        let _ = ensure_columns(src, dst, pf, opts, sink).await?;

        if should_truncate {
            if opts.disable_foreign_key_checks {
                let _ = dst.query_drop("SET FOREIGN_KEY_CHECKS = 0").await;
            }
            let trunc_sql = format!(
                "TRUNCATE TABLE {}.{}",
                quote_ident(&pf.target_schema),
                quote_ident(&pf.target_table)
            );
            dst.query_drop(&trunc_sql).await.map_err(|e| {
                Error::Other(format!(
                    "清空目标表 {}.{} 失败: {e}",
                    pf.target_schema, pf.target_table
                ))
            })?;
            sink.emit(Event::Log {
                table: Some(pf.table.clone()),
                level: "info".into(),
                message: format!(
                    "已清空目标表 {}.{} (TRUNCATE TABLE)",
                    pf.target_schema, pf.target_table
                ),
            });
        }
        return Ok(());
    }

    if !should_create {
        return Err(Error::Other(format!(
            "目标表 {}.{} 不存在且未开启自动建表",
            pf.target_schema, pf.target_table
        )));
    }

    let ddl = pf
        .create_table
        .as_ref()
        .map(|c| c.ddl.clone())
        .ok_or_else(|| Error::Preflight(format!("{} 缺少建表语句", pf.table)))?;

    dst.query_drop(&ddl).await.map_err(|e| {
        Error::DdlRewrite(format!(
            "在 {} 上建表失败: {e}\nDDL:\n{ddl}",
            pf.target_schema
        ))
    })?;

    sink.emit(Event::TableCreated {
        table: pf.table.clone(),
        ddl,
    });
    Ok(())
}

async fn source_schema_charset(conn: &mut Conn, schema: &str) -> Result<(String, String)> {
    let row: Option<(Option<String>, Option<String>)> = conn
        .exec_first(
            "SELECT DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME
             FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            (schema,),
        )
        .await?;
    match row {
        Some((Some(cs), Some(coll))) => Ok((cs, coll)),
        Some((Some(cs), None)) => Ok((cs, "utf8mb4_general_ci".into())),
        _ => Err(Error::Other("读取源库字符集失败".into())),
    }
}

/// 单表主键分片区间（用于规划切分与单元测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShardRange {
    index: usize,
    total: usize,
    lo: i128,
    /// `Some(v)` 表示带 `pk <= v` 上界；`None` 表示最后一片且未显式指定结束 ID，不封顶。
    hi: Option<i128>,
}

/// 将闭区间 `[lo, hi]` 尽量均匀地切分为最多 `threads` 个互斥子区间。
fn compute_pk_shards(
    lo: i128,
    hi: i128,
    threads: usize,
    explicit_end: bool,
) -> Vec<ShardRange> {
    if lo > hi {
        return Vec::new();
    }
    let threads = threads.max(1);
    let span: u128 = lo.abs_diff(hi).saturating_add(1);
    let count = (threads as u128).min(span) as usize;
    let base = span / (count as u128);
    let rem = span % (count as u128);

    let mut shards = Vec::with_capacity(count);
    let mut cur = lo;
    for idx in 0..count {
        let len = base + u128::from((idx as u128) < rem);
        let offset = i128::try_from(len - 1).expect("分片跨度在 i128 范围内");
        let s_lo = cur;
        let s_hi = s_lo + offset;
        if idx + 1 < count {
            cur = s_hi + 1;
        }
        let hi_bound = if idx + 1 == count && !explicit_end {
            None
        } else {
            Some(s_hi)
        };
        shards.push(ShardRange {
            index: idx,
            total: count,
            lo: s_lo,
            hi: hi_bound,
        });
    }
    shards
}

#[derive(Debug, Clone)]
enum ShardStart {
    /// 单线程模式直接在自身连接上解析起点，避免多建连接
    Deferred(CursorStart, Option<String>),
    /// 已确定的游标起点
    Fixed { cursor: Cursor, inclusive: bool },
}

#[derive(Debug, Clone)]
struct ShardSpec {
    index: usize,
    total: usize,
    start: ShardStart,
    end_id: Option<i128>,
    end_pk: Option<Value>,
    planned_lo: Option<i128>,
    planned_hi: Option<i128>,
    range_label: String,
}

fn shard_percent(
    lo: Option<i128>,
    hi: Option<i128>,
    cur: Option<i128>,
    unbounded_tail: bool,
) -> f32 {
    let (Some(lo), Some(hi), Some(cur)) = (lo, hi, cur) else {
        return 0.0;
    };
    if hi < lo || cur < lo {
        return 0.0;
    }
    let span = lo.abs_diff(hi).saturating_add(1);
    if cur >= hi && unbounded_tail {
        return 99.9;
    }
    let done = lo.abs_diff(cur).saturating_add(1).min(span);
    ((done as f64) * 100.0 / (span as f64)).clamp(0.0, 100.0) as f32
}

/// 同一张表内多分片共享的实时进度汇总器。
struct TableProgressTracker {
    total_rows: AtomicU64,
    total_batches: AtomicU64,
    shard_rps: Mutex<Vec<f64>>,
    shard_cursors: Mutex<Vec<Option<String>>>,
    last_cursor: Mutex<String>,
}

impl TableProgressTracker {
    fn new(shard_count: usize) -> Self {
        let n = shard_count.max(1);
        Self {
            total_rows: AtomicU64::new(0),
            total_batches: AtomicU64::new(0),
            shard_rps: Mutex::new(vec![0.0; n]),
            shard_cursors: Mutex::new(vec![None; n]),
            last_cursor: Mutex::new(String::new()),
        }
    }

    fn on_batch(
        &self,
        shard_idx: usize,
        returned: usize,
        rps: f64,
        cursor_disp: String,
    ) -> (u64, f64, String) {
        let total = self
            .total_rows
            .fetch_add(returned as u64, Ordering::Relaxed)
            + returned as u64;
        self.total_batches.fetch_add(1, Ordering::Relaxed);

        let sum_rps = {
            let mut rps_vec = self.shard_rps.lock().unwrap_or_else(|e| e.into_inner());
            if shard_idx < rps_vec.len() {
                rps_vec[shard_idx] = rps;
            }
            rps_vec.iter().sum()
        };

        let cursor_summary = {
            let mut cur_vec = self
                .shard_cursors
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if shard_idx < cur_vec.len() {
                cur_vec[shard_idx] = Some(cursor_disp.clone());
            }
            if cur_vec.len() <= 1 {
                cursor_disp
            } else if cur_vec.len() <= 4 {
                let parts: Vec<String> = cur_vec
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.as_ref().map(|s| format!("#{} {s}", i + 1)))
                    .collect();
                if parts.is_empty() {
                    cursor_disp
                } else {
                    parts.join(" · ")
                }
            } else {
                format!("{}线程 · 最新 {cursor_disp}", cur_vec.len())
            }
        };

        *self.last_cursor.lock().unwrap_or_else(|e| e.into_inner()) = cursor_summary.clone();
        (total, sum_rps, cursor_summary)
    }

    fn finish_shard(&self, shard_idx: usize) {
        let mut rps_vec = self.shard_rps.lock().unwrap_or_else(|e| e.into_inner());
        if shard_idx < rps_vec.len() {
            rps_vec[shard_idx] = 0.0;
        }
    }

    fn snapshot(&self) -> (u64, u64, String) {
        (
            self.total_rows.load(Ordering::Relaxed),
            self.total_batches.load(Ordering::Relaxed),
            self.last_cursor
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }
}

struct ShardRpsGuard<'a> {
    tracker: &'a TableProgressTracker,
    shard_idx: usize,
}

impl Drop for ShardRpsGuard<'_> {
    fn drop(&mut self) {
        self.tracker.finish_shard(self.shard_idx);
    }
}

/// 单表同步，返回统计。失败时按表级/全局 `continue_on_error` 决定是否上抛。
async fn run_table(
    src_cfg: ConnConfig,
    dst_cfg: ConnConfig,
    opts: TaskOptions,
    table_cfg: TableTaskConfig,
    pf: TablePreflight,
    sink: EventSink,
    cancel: CancellationToken,
    pause: PauseController,
) -> Result<TableStats> {
    let table = pf.table.clone();
    let started = Instant::now();
    let eff_continue_on_error = table_cfg.effective_continue_on_error(&opts);

    sink.emit(Event::TableStarted {
        table: table.clone(),
        columns: pf.columns.len(),
        needs_create_table: pf.needs_create_table,
    });

    let mut stats = TableStats {
        table: table.clone(),
        status: "running".into(),
        ..Default::default()
    };

    let result = run_table_inner(
        &src_cfg,
        &dst_cfg,
        &opts,
        &table_cfg,
        &pf,
        &sink,
        &cancel,
        &pause,
        &mut stats,
    )
    .await;
    stats.elapsed_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok(()) => {
            if cancel.is_cancelled() {
                stats.status = "cancelled".into();
                sink.emit(Event::Log {
                    table: Some(table),
                    level: "warn".into(),
                    message: format!("已取消，游标停在 {}", stats.cursor),
                });
            } else {
                stats.status = "done".into();
                sink.emit(Event::TableDone {
                    table,
                    rows: stats.rows,
                    elapsed_ms: stats.elapsed_ms,
                });
            }
            Ok(stats)
        }
        Err(e) => {
            stats.status = "failed".into();
            stats.error = Some(e.to_string());
            sink.emit(Event::TableFailed {
                table,
                error: e.to_string(),
            });
            if eff_continue_on_error {
                Ok(stats)
            } else {
                Err(e)
            }
        }
    }
}

async fn run_table_inner(
    src_cfg: &ConnConfig,
    dst_cfg: &ConnConfig,
    opts: &TaskOptions,
    table_cfg: &TableTaskConfig,
    pf: &TablePreflight,
    sink: &EventSink,
    cancel: &CancellationToken,
    pause: &PauseController,
    stats: &mut TableStats,
) -> Result<()> {
    let pk = pf
        .primary_key
        .clone()
        .ok_or_else(|| Error::UnsupportedPrimaryKey {
            table: pf.table.clone(),
            actual: "无主键".into(),
        })?;

    let shards = plan_table_shards(src_cfg, dst_cfg, opts, table_cfg, pf, &pk, sink).await?;
    if shards.is_empty() {
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: "同步区间为空，无需搬运数据".into(),
        });
        return Ok(());
    }

    let tracker = Arc::new(TableProgressTracker::new(shards.len()));

    if shards.len() == 1 {
        let spec = shards.into_iter().next().expect("非空");
        sync_shard(
            src_cfg, dst_cfg, opts, pf, &pk, spec, &tracker, sink, cancel, pause,
        )
        .await?;
    } else {
        let summary: Vec<String> = shards
            .iter()
            .map(|s| format!("#{} {}", s.index + 1, s.range_label))
            .collect();
        sink.emit(Event::TableShardsPlanned {
            table: pf.table.clone(),
            shards: shards
                .iter()
                .map(|s| ShardInfo {
                    index: s.index,
                    total: s.total,
                    range: s.range_label.clone(),
                })
                .collect(),
        });
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!(
                "启用 {} 线程并行分片: {}",
                shards.len(),
                summary.join(", ")
            ),
        });

        let table_cancel = cancel.child_token();
        let mut futs = FuturesUnordered::new();

        for spec in shards {
            let shard_index = spec.index;
            let src_cfg = src_cfg.clone();
            let dst_cfg = dst_cfg.clone();
            let opts = opts.clone();
            let pf = pf.clone();
            let pk = pk.clone();
            let tracker = Arc::clone(&tracker);
            let sink = sink.clone();
            let shard_cancel = table_cancel.clone();
            let pause = pause.clone();

            futs.push(tokio::spawn(async move {
                let res = sync_shard(
                    &src_cfg,
                    &dst_cfg,
                    &opts,
                    &pf,
                    &pk,
                    spec,
                    &tracker,
                    &sink,
                    &shard_cancel,
                    &pause,
                )
                .await;
                if let Err(ref e) = res {
                    if !shard_cancel.is_cancelled() {
                        sink.emit(Event::ShardFailed {
                            table: pf.table.clone(),
                            shard_index,
                            error: e.to_string(),
                        });
                    }
                }
                res
            }));
        }

        let mut first_err: Option<Error> = None;
        while let Some(joined) = futs.next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                        table_cancel.cancel();
                    }
                }
                Err(join_err) => {
                    if first_err.is_none() {
                        first_err = Some(Error::Other(format!("分片工作线程异常: {join_err}")));
                        table_cancel.cancel();
                    }
                }
            }
        }

        if !cancel.is_cancelled() {
            if let Some(err) = first_err {
                return Err(err);
            }
        }
    }

    let (rows, batches, last_cursor) = tracker.snapshot();
    stats.rows = rows;
    stats.batches = batches;
    stats.cursor = last_cursor;
    Ok(())
}

async fn plan_table_shards(
    src_cfg: &ConnConfig,
    dst_cfg: &ConnConfig,
    opts: &TaskOptions,
    table_cfg: &TableTaskConfig,
    pf: &TablePreflight,
    pk: &str,
    sink: &EventSink,
) -> Result<Vec<ShardSpec>> {
    let eff_threads = table_cfg.effective_table_parallelism(opts);
    let eff_cursor_start = table_cfg.effective_cursor_start(opts);
    let eff_start_id = table_cfg
        .effective_start_id(opts)
        .filter(|s| !s.trim().is_empty());
    let eff_end_id = table_cfg
        .effective_end_id(opts)
        .filter(|s| !s.trim().is_empty());

    let (end_id, end_pk) = match &eff_end_id {
        Some(s) => {
            let v = parse_cursor_value(s, pf.primary_key_unsigned)?;
            let as_int = value_as_i128(&v)
                .ok_or_else(|| Error::Config(format!("结束游标 `{s}` 不是合法整数")))?;
            (Some(as_int), Some(v))
        }
        None => (None, None),
    };

    if table_cfg.has_cursor_override() {
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!(
                "本表独立游标：{} 起 {}，止 {}",
                match eff_cursor_start {
                    CursorStart::Full => "全量",
                    CursorStart::Resume => "续传",
                    CursorStart::Custom => "自定义",
                },
                eff_start_id.as_deref().unwrap_or("无"),
                eff_end_id.as_deref().unwrap_or("不限"),
            ),
        });
    }

    if let Some(f) = &pf.filter {
        let trimmed = f.trim();
        if !trimmed.is_empty() {
            sink.emit(Event::Log {
                table: Some(pf.table.clone()),
                level: "info".into(),
                message: format!("附加条件过滤: {trimmed}"),
            });
        }
    }

    if eff_threads <= 1 {
        return Ok(vec![ShardSpec {
            index: 0,
            total: 1,
            start: ShardStart::Deferred(eff_cursor_start, eff_start_id),
            end_id,
            end_pk,
            planned_lo: None,
            planned_hi: end_id,
            range_label: String::new(),
        }]);
    }

    // 多线程分片：仅在需要解析未知端时连接查 MIN/MAX(pk)（均走主键极值 O(1)）。
    let explicit_start = if eff_cursor_start == CursorStart::Custom {
        match &eff_start_id {
            Some(s) => {
                let v = parse_cursor_value(s, pf.primary_key_unsigned)?;
                Some(
                    value_as_i128(&v)
                        .ok_or_else(|| Error::Config(format!("起始游标 `{s}` 不是合法整数")))?,
                )
            }
            None => None,
        }
    } else {
        None
    };

    let need_src_min = match eff_cursor_start {
        CursorStart::Full => true,
        CursorStart::Custom => explicit_start.is_none(),
        CursorStart::Resume => false,
    };
    let explicit_end = end_id.is_some();
    let need_src_max = !explicit_end;

    let mut resume_lo: Option<i128> = None;
    let mut need_src_min_fallback = need_src_min;

    if eff_cursor_start == CursorStart::Resume {
        let mut dst = connect(dst_cfg, "目标").await?;
        let dst_max = read_max_pk(&mut dst, &pf.target_schema, &pf.target_table, pk).await?;
        let _ = dst.disconnect().await;
        match dst_max {
            Some(v) => {
                let m = value_as_i128(&v)
                    .ok_or_else(|| Error::Other(format!("目标表最大主键不是合法整数")))?;
                resume_lo = Some(m.saturating_add(1));
            }
            None => {
                need_src_min_fallback = true;
            }
        }
    }

    let (src_min, src_max) = if need_src_min_fallback || need_src_max {
        let mut src = connect(src_cfg, "源").await?;
        let bounds = match (need_src_min_fallback, need_src_max) {
            (true, true) => read_pk_bounds(&mut src, &pf.source_schema, &pf.table, pk).await?,
            (true, false) => {
                (
                    read_pk_bound(&mut src, &pf.source_schema, &pf.table, pk, false).await?,
                    None,
                )
            }
            (false, true) => (
                None,
                read_pk_bound(&mut src, &pf.source_schema, &pf.table, pk, true).await?,
            ),
            (false, false) => (None, None),
        };
        let _ = src.disconnect().await;
        bounds
    } else {
        (None, None)
    };

    let lo = match (eff_cursor_start, explicit_start, resume_lo) {
        (CursorStart::Custom, Some(s), _) => s,
        (CursorStart::Resume, _, Some(r)) => r,
        _ => match src_min.as_ref().and_then(value_as_i128) {
            Some(v) => v,
            None => return Ok(Vec::new()),
        },
    };

    let hi = match end_id {
        Some(e) => e,
        None => match src_max.as_ref().and_then(value_as_i128) {
            Some(v) => v,
            None => return Ok(Vec::new()),
        },
    };

    let ranges = compute_pk_shards(lo, hi, eff_threads, explicit_end);
    if ranges.is_empty() {
        return Ok(Vec::new());
    }

    let mut specs = Vec::with_capacity(ranges.len());
    for r in ranges {
        let start_val = i128_to_pk_value(r.lo, pf.primary_key_unsigned)?;
        let end_val = match r.hi {
            Some(h) => Some(i128_to_pk_value(h, pf.primary_key_unsigned)?),
            None => None,
        };
        let range_label = match r.hi {
            Some(h) => format!("[{}..={}]", r.lo, h),
            None => format!("[{}..不限]", r.lo),
        };
        specs.push(ShardSpec {
            index: r.index,
            total: r.total,
            start: ShardStart::Fixed {
                cursor: Cursor(Some(start_val)),
                inclusive: true,
            },
            end_id: r.hi,
            end_pk: end_val,
            planned_lo: Some(r.lo),
            planned_hi: Some(r.hi.unwrap_or(hi)),
            range_label,
        });
    }
    Ok(specs)
}

async fn sync_shard(
    src_cfg: &ConnConfig,
    dst_cfg: &ConnConfig,
    opts: &TaskOptions,
    pf: &TablePreflight,
    pk: &str,
    spec: ShardSpec,
    tracker: &TableProgressTracker,
    sink: &EventSink,
    cancel: &CancellationToken,
    pause: &PauseController,
) -> Result<()> {
    let _guard = ShardRpsGuard {
        tracker,
        shard_idx: spec.index,
    };
    let shard_started = Instant::now();
    let shard_tag = if spec.total > 1 {
        format!("[分片 {}/{}] ", spec.index + 1, spec.total)
    } else {
        String::new()
    };

    let mut src = connect(src_cfg, "源").await?;
    let mut dst = connect(dst_cfg, "目标").await?;

    apply_session_opts(&mut src, opts, false).await?;
    apply_session_opts(&mut dst, opts, true).await?;

    let dst_ver = introspect::server_version(&mut dst).await?;
    let dst_is_mariadb = introspect::is_mariadb(&mut dst).await?;
    let packet = introspect::max_allowed_packet(&mut dst).await?;
    let budget = (packet as f64 * PACKET_BUDGET_RATIO) as u64;

    // 按往返延迟挑初始批大小。远程库 RTT 高，从 1000 起步再慢慢放大，
    // 光爬升阶段就要浪费几十次往返；直接按延迟给个合理起点。
    //
    // 注意：这里绝不能 max() 覆盖用户设的值——用户明确填了批大小（比如
    // 关闭自适应想要固定 137 行/批），就必须原样生效。只有用户没设
    // （等于默认值）时才用 RTT 推荐值，否则用户填多少就是多少。
    let initial_batch = {
        let t0 = Instant::now();
        let _ = src.query_drop("SELECT 1").await;
        let rtt_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let default_sized = opts.batch_size == crate::config::default_batch_size();
        let recommended = match rtt_ms {
            r if r < 2.0 => 2000usize,
            r if r < 10.0 => 2000usize,
            r if r < 30.0 => 5000usize,
            _ => 10000usize,
        };
        if default_sized {
            // 用户没动过批大小：用 RTT 推荐值，避免远程库爬升太慢
            opts.batch_size.max(recommended)
        } else {
            // 用户设过：完全尊重，不放大不缩小
            opts.batch_size
        }
    };

    let (cursor, cursor_inclusive) = match spec.start {
        ShardStart::Fixed { cursor, inclusive } => (cursor, inclusive),
        ShardStart::Deferred(eff_cursor_start, eff_start_id) => match eff_cursor_start {
            CursorStart::Full => (Cursor::start(), false),
            CursorStart::Resume => {
                let max = read_max_pk(&mut dst, &pf.target_schema, &pf.target_table, pk).await?;
                (Cursor::from_max_pk(max), false)
            }
            CursorStart::Custom => match &eff_start_id {
                Some(s) => (
                    Cursor(Some(parse_cursor_value(s, pf.primary_key_unsigned)?)),
                    true, // 自定义起始ID包含此行（闭区间 [start_id, end_id]）
                ),
                _ => (Cursor::start(), false),
            },
        },
    };

    let pk_index = pf
        .columns
        .iter()
        .position(|c| c == pk)
        .ok_or_else(|| Error::Other(format!("主键 {pk} 不在同步列中")))?;

    let mut run = TableRun {
        pk: pk.to_string(),
        pk_index,
        columns: pf.columns.clone(),
        cursor,
        cursor_inclusive,
        end_id: spec.end_id,
        end_pk: spec.end_pk,
        batch_size: initial_batch.clamp(MIN_BATCH, opts.max_batch_size.max(MIN_BATCH)),
        src_schema: pf.source_schema.clone(),
        dst_schema: pf.target_schema.clone(),
        src_table: pf.table.clone(),
        dst_table: pf.target_table.clone(),
        filter: pf.filter.clone(),
    };

    if spec.total == 1 && !run.cursor.is_start() {
        let op = if run.cursor_inclusive { ">=" } else { ">" };
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!("起始游标 {} {op} {}", run.pk, run.cursor.display()),
        });
    }

    let use_new_upsert = !dst_is_mariadb && dst_ver >= (8, 0, 19);
    // 占位符上限：行数 × 列数 ≤ 65535。
    // 必须约束初始批大小——否则宽表第一批就会报 1390
    // (Prepared statement contains too many placeholders)。
    let placeholder_cap = (MAX_PLACEHOLDERS / run.columns.len().max(1)).max(1);
    run.batch_size = run.batch_size.min(placeholder_cap);

    let mut sql_cache: Option<(usize, String)> = None;
    let mut shard_rows: u64 = 0;

    loop {
        if cancel.is_cancelled() {
            break;
        }
        pause.check_pause(cancel).await;
        if cancel.is_cancelled() {
            break;
        }

        let start = Instant::now();
        // 记录本批实际使用的批大小。自适应会改动 run.batch_size，
        // 若用改动后的值判断「是否已读完」，批大小翻倍时会误判提前结束而静默丢行。
        let fetch_size = run.batch_size;
        let (rows, wire_bytes) =
            fetch_batch_retry(&mut src, src_cfg, opts, &run, sink, &pf.table, &shard_tag).await?;
        if rows.is_empty() {
            break;
        }
        let returned = rows.len();

        // 本批最后一行的 pk，作为下一批游标。ORDER BY 保证它是本批最大值。
        let last_pk = rows
            .last()
            .map(|r: &Vec<Value>| r[run.pk_index].clone())
            .expect("非空");
        if matches!(last_pk, Value::NULL) {
            return Err(Error::Other(format!(
                "{shard_tag}主键 {} 出现 NULL，无法用游标推进",
                run.pk
            )));
        }

        // 包大小保护：超预算就降级
        let single_row_bytes = wire_bytes / returned.max(1) as u64;
        if returned > 1 && wire_bytes > budget {
            run.batch_size = shrink_for_budget(run.batch_size, single_row_bytes, budget);
            sink.emit(Event::Warning {
                table: Some(pf.table.clone()),
                message: format!(
                    "{shard_tag}本批约 {:.1}MB 超过包预算 {:.1}MB，批大小降至 {}",
                    wire_bytes as f64 / 1e6,
                    budget as f64 / 1e6,
                    run.batch_size
                ),
            });
            continue;
        }

        let write_result = write_batch_retry(
            &mut dst,
            dst_cfg,
            opts,
            &run,
            &rows,
            use_new_upsert,
            sink,
            &pf.table,
            &shard_tag,
            &mut sql_cache,
        )
        .await;

        match write_result {
            Ok(()) => {}
            Err(e) if e.should_shrink_batch() => {
                if returned <= 1 || run.batch_size <= MIN_BATCH {
                    return Err(e);
                }
                run.batch_size = (run.batch_size / 2).max(MIN_BATCH);
                sink.emit(Event::Warning {
                    table: Some(pf.table.clone()),
                    message: format!("{shard_tag}{e}，批大小降至 {}", run.batch_size),
                });
                continue;
            }
            Err(e) => return Err(e),
        }

        run.cursor.advance(last_pk);
        run.cursor_inclusive = false;
        shard_rows += returned as u64;

        let elapsed = start.elapsed();
        let rps = if elapsed.as_secs_f64() > 0.0 {
            returned as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        if opts.adaptive_batch {
            // 每批只翻一倍的话，batch=1000 要到 1 万需要 4 批（含 4 次网络往返），
            // 远程库上这几批的代价不小。这里按「每批约 300ms」的目标直接算出
            // 合适的批大小，一步到位；之后只用小幅微调。
            let ms = elapsed.as_millis().max(1) as f64;
            let cap = opts.max_batch_size.min(placeholder_cap);
            let target = if ms < 300.0 {
                // 按当前吞吐外推：这一批用 ms 毫秒处理了 returned 行，
                // 想让它涨到 300ms，行数应放大 300/ms 倍。
                ((returned as f64) * (300.0 / ms)).ceil() as usize
            } else if ms > 1000.0 {
                ((returned as f64) * (300.0 / ms)).floor() as usize
            } else {
                run.batch_size
            };
            run.batch_size = target.clamp(MIN_BATCH, cap);
        }

        let cur_disp = run.cursor.display();
        let cur_int = run.cursor.as_i128();
        let (total_rows, sum_rps, cursor_summary) =
            tracker.on_batch(spec.index, returned, rps, cur_disp.clone());

        sink.emit(Event::BatchDone {
            table: pf.table.clone(),
            rows: returned as u64,
            total_rows,
            cursor: cursor_summary,
            rows_per_sec: sum_rps,
            batch_size: run.batch_size,
            elapsed_ms: elapsed.as_millis() as u64,
        });

        if spec.total > 1 {
            let pct = shard_percent(
                spec.planned_lo,
                spec.planned_hi,
                cur_int,
                spec.end_id.is_none(),
            );
            sink.emit(Event::ShardBatchDone {
                table: pf.table.clone(),
                shard_index: spec.index,
                rows: returned as u64,
                total_rows: shard_rows,
                cursor: cur_disp,
                rows_per_sec: rps,
                percent: pct,
            });
        }

        if let (Some(end), Some(cur)) = (run.end_id, run.cursor.as_i128()) {
            if cur >= end {
                if spec.total == 1 {
                    sink.emit(Event::Log {
                        table: Some(pf.table.clone()),
                        level: "info".into(),
                        message: format!("已到达结束游标 {end}"),
                    });
                }
                break;
            }
        }

        if is_last_batch(returned, fetch_size) {
            // 本批没取满，说明源表已读完。必须用 fetch_size 而非当前的 run.batch_size：
            // 后面自适应可能刚把 batch_size 放大，拿放大后的值比较会导致误判提前结束。
            break;
        }
    }

    if spec.total > 1 && !cancel.is_cancelled() {
        sink.emit(Event::ShardDone {
            table: pf.table.clone(),
            shard_index: spec.index,
            rows: shard_rows,
            elapsed_ms: shard_started.elapsed().as_millis() as u64,
        });
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!(
                "{shard_tag}{} 完成，同步 {shard_rows} 行",
                spec.range_label
            ),
        });
    }

    let _ = src.disconnect().await;
    let _ = dst.disconnect().await;
    Ok(())
}

fn shrink_for_budget(current: usize, single_row_bytes: u64, budget: u64) -> usize {
    if single_row_bytes == 0 {
        return current;
    }
    let fit = (budget / single_row_bytes) as usize;
    // 超预算时降到能装下的行数，但不放大、不低于下限
    fit.clamp(MIN_BATCH, current.max(MIN_BATCH)).min(current)
}

/// 判断本批是不是最后一批。
///
/// 必须拿「本批实际请求的条数」来比，而不是自适应调整之后的批大小。
/// 历史 bug：自适应在读取后把 batch_size 从 1000 翻到 2000，
/// 再用 `returned(1000) < 2000` 判断，就会误认为源表读完了，
/// 10000 行的表只同步了 1000 行且**不报任何错**。
fn is_last_batch(returned: usize, fetch_size: usize) -> bool {
    returned < fetch_size
}

/// 读取一批数据，带重试与自动重连。
async fn fetch_batch_retry(
    src: &mut Conn,
    src_cfg: &ConnConfig,
    opts: &TaskOptions,
    run: &TableRun,
    sink: &EventSink,
    table: &str,
    shard_tag: &str,
) -> Result<(Vec<Vec<Value>>, u64)> {
    let mut attempt = 0;
    loop {
        match fetch_batch(src, run).await {
            Ok(res) => return Ok(res),
            Err(e) if attempt < opts.max_retries && e.is_retryable() => {
                attempt += 1;
                sink.emit(Event::Warning {
                    table: Some(table.to_string()),
                    message: format!(
                        "{shard_tag}源端读取异常 ({e})，正在第 {attempt}/{} 次重试...",
                        opts.max_retries
                    ),
                });
                if matches!(e.code(), Some(2006 | 2013)) || matches!(e, Error::Io(_)) {
                    if let Ok(mut new_conn) = connect(src_cfg, "源").await {
                        let _ = apply_session_opts(&mut new_conn, opts, false).await;
                        *src = new_conn;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(200 * attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 写入一批数据，带重试、自动重连与缩批支持。
async fn write_batch_retry(
    dst: &mut Conn,
    dst_cfg: &ConnConfig,
    opts: &TaskOptions,
    run: &TableRun,
    rows: &[Vec<Value>],
    use_new_upsert: bool,
    sink: &EventSink,
    table: &str,
    shard_tag: &str,
    sql_cache: &mut Option<(usize, String)>,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        match write_batch(dst, run, rows, opts, use_new_upsert, sql_cache).await {
            Ok(()) => return Ok(()),
            Err(e) if e.should_shrink_batch() => {
                // 由上层缩批逻辑处理
                return Err(e);
            }
            Err(e) if attempt < opts.max_retries && e.is_retryable() => {
                attempt += 1;
                sink.emit(Event::Warning {
                    table: Some(table.to_string()),
                    message: format!(
                        "{shard_tag}目标端写入异常 ({e})，正在第 {attempt}/{} 次重试...",
                        opts.max_retries
                    ),
                });
                if matches!(e.code(), Some(2006 | 2013)) || matches!(e, Error::Io(_)) {
                    if let Ok(mut new_conn) = connect(dst_cfg, "目标").await {
                        let _ = apply_session_opts(&mut new_conn, opts, true).await;
                        *dst = new_conn;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(300 * attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// 读取一批数据。返回 (行, 估算线上字节数)。
async fn fetch_batch(src: &mut Conn, run: &TableRun) -> Result<(Vec<Vec<Value>>, u64)> {
    let cols = run
        .columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");

    // 结束游标必须下推到 SQL 的 WHERE 里。
    // 只在写完一批之后再比较 end_id 的话，最后一批会整批写进去，
    // 最多超出 end_id 边界 batch_size-1 行 —— 这是实打实的数据错误。
    let mut conds: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();

    if let Some(v) = &run.cursor.0 {
        let op = if run.cursor_inclusive { ">=" } else { ">" };
        conds.push(format!("{} {op} ?", quote_ident(&run.pk)));
        params.push(v.clone());
    }
    if let Some(end) = &run.end_pk {
        conds.push(format!("{} <= ?", quote_ident(&run.pk)));
        params.push(end.clone());
    }
    if let Some(filter) = &run.filter {
        let trimmed = filter.trim();
        if !trimmed.is_empty() {
            conds.push(format!("({trimmed})"));
        }
    }

    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conds.join(" AND "))
    };

    let sql = format!(
        "SELECT {cols} FROM {}{where_clause} ORDER BY {} ASC LIMIT ?",
        qualified(&run.src_schema, &run.src_table),
        quote_ident(&run.pk)
    );
    params.push(Value::UInt(run.batch_size as u64));

    let params = Params::Positional(params);

    let read_err = |e: mysql_async::Error| {
        Error::Other(format!(
            "读取源表 {}.{} 失败: {e}",
            run.src_schema, run.src_table
        ))
    };
    let mut result = src.exec_iter(sql, params).await.map_err(read_err)?;
    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(run.batch_size.min(4096));
    let mut bytes: u64 = 0;

    while let Some(row) = result.next().await.map_err(read_err)? {
        let values = row.unwrap();
        for v in &values {
            bytes += value::value_wire_size(v) as u64;
        }
        rows.push(values);
    }
    result.drop_result().await.map_err(read_err)?;

    Ok((rows, bytes))
}

fn build_write_sql(
    run: &TableRun,
    row_count: usize,
    opts: &TaskOptions,
    use_new_upsert: bool,
) -> String {
    let ncols = run.columns.len();
    let cols = run
        .columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let row_placeholder = format!(
        "({})",
        std::iter::repeat("?")
            .take(ncols)
            .collect::<Vec<_>>()
            .join(",")
    );
    let values_clause = std::iter::repeat(row_placeholder.as_str())
        .take(row_count)
        .collect::<Vec<_>>()
        .join(",");

    match opts.write_mode {
        WriteMode::Insert => format!(
            "INSERT INTO {} ({cols}) VALUES {values_clause}",
            qualified(&run.dst_schema, &run.dst_table)
        ),
        WriteMode::InsertIgnore => format!(
            "INSERT IGNORE INTO {} ({cols}) VALUES {values_clause}",
            qualified(&run.dst_schema, &run.dst_table)
        ),
        WriteMode::Upsert => {
            // UPDATE 子句绝不能包含主键列，否则会试图改主键。
            let updatable: Vec<&String> = run
                .columns
                .iter()
                .filter(|c| c.as_str() != run.pk)
                .collect();
            let update_clause = if updatable.is_empty() {
                // 只有主键列：用主键自赋值制造一个 no-op UPDATE
                format!("{}={}", quote_ident(&run.pk), quote_ident(&run.pk))
            } else if use_new_upsert {
                updatable
                    .iter()
                    .map(|c| format!("{} = new.{}", quote_ident(c), quote_ident(c)))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                updatable
                    .iter()
                    .map(|c| format!("{} = VALUES({})", quote_ident(c), quote_ident(c)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            let alias = if use_new_upsert { " AS new" } else { "" };
            format!(
                "INSERT INTO {} ({cols}) VALUES {values_clause}{alias} ON DUPLICATE KEY UPDATE {update_clause}",
                qualified(&run.dst_schema, &run.dst_table)
            )
        }
    }
}

/// 写入一批。一个事务，一条多行 INSERT。
async fn write_batch(
    dst: &mut Conn,
    run: &TableRun,
    rows: &[Vec<Value>],
    opts: &TaskOptions,
    use_new_upsert: bool,
    sql_cache: &mut Option<(usize, String)>,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let ncols = run.columns.len();
    let mut params: Vec<Value> = Vec::with_capacity(rows.len() * ncols);
    for r in rows {
        debug_assert_eq!(r.len(), ncols, "列数不一致");
        params.extend(r.iter().cloned());
    }

    let sql = if let Some((cached_len, cached_sql)) = sql_cache {
        if *cached_len == rows.len() {
            cached_sql.clone()
        } else {
            let new_sql = build_write_sql(run, rows.len(), opts, use_new_upsert);
            *sql_cache = Some((rows.len(), new_sql.clone()));
            new_sql
        }
    } else {
        let new_sql = build_write_sql(run, rows.len(), opts, use_new_upsert);
        *sql_cache = Some((rows.len(), new_sql.clone()));
        new_sql
    };

    // 一条语句完成「开事务 + 写入 + 提交」，把每批 3 次往返压成 1 次。
    // 远程库上 RTT 是主要成本，这一下能省掉 2/3 的写侧往返。
    //
    // 注意：MySQL 的 autocommit 默认开启，单条语句本身就是原子的，
    // 所以这里其实不需要显式事务 —— 之前每批发 START/COMMIT 是多余的开销。
    match dst.exec_drop(sql, Params::Positional(params)).await {
        Ok(()) => Ok(()),
        Err(e) => Err(Error::Other(format!(
            "写入目标表 {}.{} 失败: {e}",
            run.dst_schema, run.dst_table
        ))),
    }
}

async fn apply_session_opts(conn: &mut Conn, opts: &TaskOptions, is_target: bool) -> Result<()> {
    // 首版只在目标端放开这些开关：源端应保持只读的默认行为。
    if !is_target {
        return Ok(());
    }
    if opts.disable_foreign_key_checks {
        conn.query_drop("SET SESSION foreign_key_checks = 0")
            .await?;
    }
    if opts.relax_sql_mode {
        conn.query_drop("SET SESSION sql_mode = ''").await?;
    }
    // 大字段传输时避免超时
    conn.query_drop("SET SESSION net_write_timeout = 600")
        .await?;
    conn.query_drop("SET SESSION net_read_timeout = 600")
        .await?;
    Ok(())
}

async fn read_max_pk(
    conn: &mut Conn,
    schema: &str,
    table: &str,
    pk: &str,
) -> Result<Option<Value>> {
    read_pk_bound(conn, schema, table, pk, true).await
}

async fn read_pk_bound(
    conn: &mut Conn,
    schema: &str,
    table: &str,
    pk: &str,
    is_max: bool,
) -> Result<Option<Value>> {
    let agg = if is_max { "MAX" } else { "MIN" };
    let sql = format!(
        "SELECT {agg}({}) FROM {}",
        quote_ident(pk),
        qualified(schema, table)
    );
    let row: Option<(Option<Value>,)> = conn.query_first(sql).await?;
    Ok(row
        .and_then(|(v,)| v)
        .filter(|v| !matches!(v, Value::NULL)))
}

async fn read_pk_bounds(
    conn: &mut Conn,
    schema: &str,
    table: &str,
    pk: &str,
) -> Result<(Option<Value>, Option<Value>)> {
    let qpk = quote_ident(pk);
    let sql = format!(
        "SELECT MIN({qpk}), MAX({qpk}) FROM {}",
        qualified(schema, table)
    );
    let row: Option<(Option<Value>, Option<Value>)> = conn.query_first(sql).await?;
    Ok(match row {
        Some((min_v, max_v)) => (
            min_v.filter(|v| !matches!(v, Value::NULL)),
            max_v.filter(|v| !matches!(v, Value::NULL)),
        ),
        None => (None, None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_batch_uses_fetch_size_not_adapted_size() {
        // 本批请求 1000，返回 1000 → 不是最后一批（即使自适应已把 batch_size 调到 2000）
        assert!(!is_last_batch(1000, 1000));
        // 本批请求 1000，返回 999 → 是最后一批
        assert!(is_last_batch(999, 1000));
        // 回归：此前误用调整后的 2000 比较，1000 < 2000 被判成最后一批
        assert!(!is_last_batch(1000, 1000), "批大小翻倍后不得误判结束");
    }

    #[test]
    fn budget_shrink_respects_floor_and_never_grows() {
        // 单行 10MB，预算 4MB → 装不下，保底 100
        assert_eq!(shrink_for_budget(1000, 10_000_000, 4_000_000), MIN_BATCH);
        // 单行 1KB，预算 4MB → 能装 4000 行，但不能超过当前值
        assert_eq!(shrink_for_budget(1000, 1_000, 4_000_000), 1000);
        // 单行 2KB，预算 4MB → 2000 行，当前 5000 → 降到 2000
        assert_eq!(shrink_for_budget(5000, 2_000, 4_000_000), 2000);
        assert_eq!(shrink_for_budget(5000, 0, 4_000_000), 5000);
    }

    #[test]
    fn compute_pk_shards_even_split_1_to_10000() {
        let shards = compute_pk_shards(1, 10_000, 4, true);
        assert_eq!(
            shards,
            vec![
                ShardRange {
                    index: 0,
                    total: 4,
                    lo: 1,
                    hi: Some(2500)
                },
                ShardRange {
                    index: 1,
                    total: 4,
                    lo: 2501,
                    hi: Some(5000)
                },
                ShardRange {
                    index: 2,
                    total: 4,
                    lo: 5001,
                    hi: Some(7500)
                },
                ShardRange {
                    index: 3,
                    total: 4,
                    lo: 7501,
                    hi: Some(10_000)
                },
            ]
        );
    }

    #[test]
    fn compute_pk_shards_handles_remainder_negative_u64_and_small_span() {
        // 余数均匀分配到前几个分片，且未显式指定 end_id 时最后一片不封顶
        let s1 = compute_pk_shards(1, 10, 4, false);
        assert_eq!(
            s1,
            vec![
                ShardRange {
                    index: 0,
                    total: 4,
                    lo: 1,
                    hi: Some(3)
                },
                ShardRange {
                    index: 1,
                    total: 4,
                    lo: 4,
                    hi: Some(6)
                },
                ShardRange {
                    index: 2,
                    total: 4,
                    lo: 7,
                    hi: Some(8)
                },
                ShardRange {
                    index: 3,
                    total: 4,
                    lo: 9,
                    hi: None
                },
            ]
        );

        // 负数跨零区间
        let s2 = compute_pk_shards(-10, 9, 4, true);
        assert_eq!(s2[0].lo, -10);
        assert_eq!(s2[0].hi, Some(-6));
        assert_eq!(s2[3].lo, 5);
        assert_eq!(s2[3].hi, Some(9));

        // 接近 u64::MAX 区间不溢出
        let hi = u64::MAX as i128;
        let lo = hi - 99;
        let s3 = compute_pk_shards(lo, hi, 4, true);
        assert_eq!(s3.len(), 4);
        assert_eq!(s3[0].lo, lo);
        assert_eq!(s3[3].hi, Some(hi));

        // 区间比线程数小：仅创建 span 个分片
        let s4 = compute_pk_shards(5, 6, 4, true);
        assert_eq!(s4.len(), 2);
        assert_eq!(s4[0].lo, 5);
        assert_eq!(s4[0].hi, Some(5));
        assert_eq!(s4[1].lo, 6);
        assert_eq!(s4[1].hi, Some(6));

        // 空区间
        assert!(compute_pk_shards(10, 5, 4, true).is_empty());
    }

    #[test]
    fn progress_tracker_aggregates_shards_rps_and_rows() {
        let tracker = TableProgressTracker::new(4);
        let (tot1, rps1, cur1) = tracker.on_batch(0, 500, 1000.0, "500".into());
        assert_eq!(tot1, 500);
        assert_eq!(rps1, 1000.0);
        assert_eq!(cur1, "#1 500");

        let (tot2, rps2, cur2) = tracker.on_batch(2, 600, 1500.0, "5600".into());
        assert_eq!(tot2, 1100);
        assert_eq!(rps2, 2500.0);
        assert_eq!(cur2, "#1 500 · #3 5600");

        tracker.finish_shard(0);
        let (tot3, rps3, _) = tracker.on_batch(2, 400, 1200.0, "6000".into());
        assert_eq!(tot3, 1500);
        assert_eq!(rps3, 1200.0);
    }

    #[test]
    fn shard_percent_tracks_range_and_unbounded_tail() {
        assert_eq!(shard_percent(Some(1), Some(2500), Some(1250), false), 50.0);
        assert_eq!(shard_percent(Some(1), Some(2500), Some(2500), false), 100.0);
        assert_eq!(shard_percent(Some(7501), Some(10000), Some(10005), true), 99.9);
        assert_eq!(shard_percent(Some(1), Some(2500), Some(0), false), 0.0);
    }
}
