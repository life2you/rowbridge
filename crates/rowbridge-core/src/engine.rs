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

use std::sync::Arc;
use std::time::Instant;

use futures_util::stream::{FuturesUnordered, StreamExt};
use mysql_async::prelude::*;
use mysql_async::{Conn, Params, Value};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::{ConnConfig, CursorStart, TaskConfig, TaskOptions, WriteMode};
use crate::introspect;
use crate::preflight::{self, TablePreflight};
use crate::value::{self, parse_cursor_value, qualified, quote_ident, Cursor};
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
    TaskStarted { tables: usize },
    TableStarted { table: String, columns: usize, needs_create_table: bool },
    TableCreated { table: String, ddl: String },
    BatchDone {
        table: String,
        rows: u64,
        total_rows: u64,
        cursor: String,
        rows_per_sec: f64,
        batch_size: usize,
        elapsed_ms: u64,
    },
    TableDone { table: String, rows: u64, elapsed_ms: u64 },
    TableFailed { table: String, error: String },
    Warning { table: Option<String>, message: String },
    Log { table: Option<String>, level: String, message: String },
    TaskDone { total_rows: u64, succeeded: usize, failed: usize, elapsed_ms: u64 },
    Cancelled,
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
    end_id: Option<i128>,
    batch_size: usize,
    src_schema: String,
    dst_schema: String,
    src_table: String,
    dst_table: String,
}

/// 执行整个同步任务（多表并行）。
pub async fn run_task(
    cfg: TaskConfig,
    preflight: &preflight::PreflightReport,
    sink: EventSink,
    cancel: CancellationToken,
) -> Result<TaskReport> {
    let started = Instant::now();
    let opts = cfg.options.clone();
    let parallelism = opts.parallelism.max(1);

    let mut runnable: Vec<TablePreflight> = preflight
        .syncable_tables()
        .into_iter()
        .cloned()
        .collect();
    if runnable.is_empty() {
        return Err(Error::Preflight("没有可同步的表（预检全部未通过）".into()));
    }
    runnable.sort_by(|a, b| a.table.cmp(&b.table));

    sink.emit(Event::TaskStarted {
        tables: runnable.len(),
    });

    // 建库/建表串行执行，避免并发 DDL 冲突。
    let mut dst = connect(&cfg.target, "目标").await?;
    let mut ready: Vec<TablePreflight> = Vec::new();
    for pf in runnable {
        if cancel.is_cancelled() {
            break;
        }
        match prepare_table(&mut dst, &pf, &opts, &sink).await {
            Ok(()) => ready.push(pf),
            Err(e) => {
                sink.emit(Event::TableFailed {
                    table: pf.table.clone(),
                    error: e.to_string(),
                });
                if !opts.continue_on_error {
                    drop(dst);
                    return Err(e);
                }
            }
        }
    }
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

        futs.push(tokio::spawn(async move {
            let _permit = permit;
            run_table(src_cfg, dst_cfg, opts, pf, sink, cancel).await
        }));
    }

    let mut report = TaskReport::default();
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
async fn prepare_table(
    dst: &mut Conn,
    pf: &TablePreflight,
    opts: &TaskOptions,
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
        // 沿用源库的字符集，避免建出来的库用错默认 charset。
        let (charset, collation) = source_schema_charset(dst, &pf.source_schema)
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
        return Ok(());
    }

    if !opts.create_table {
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

async fn source_schema_charset(
    conn: &mut Conn,
    schema: &str,
) -> Result<(String, String)> {
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

/// 单表同步，返回统计。失败时按 `continue_on_error` 决定是否上抛。
async fn run_table(
    src_cfg: ConnConfig,
    dst_cfg: ConnConfig,
    opts: TaskOptions,
    pf: TablePreflight,
    sink: EventSink,
    cancel: CancellationToken,
) -> Result<TableStats> {
    let table = pf.table.clone();
    let started = Instant::now();

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

    let result = sync_one(&src_cfg, &dst_cfg, &opts, &pf, &sink, &cancel, &mut stats).await;
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
            if opts.continue_on_error {
                Ok(stats)
            } else {
                Err(e)
            }
        }
    }
}

async fn sync_one(
    src_cfg: &ConnConfig,
    dst_cfg: &ConnConfig,
    opts: &TaskOptions,
    pf: &TablePreflight,
    sink: &EventSink,
    cancel: &CancellationToken,
    stats: &mut TableStats,
) -> Result<()> {
    let pk = pf
        .primary_key
        .clone()
        .ok_or_else(|| Error::UnsupportedPrimaryKey {
            table: pf.table.clone(),
            actual: "无主键".into(),
        })?;

    let mut src = connect(src_cfg, "源").await?;
    let mut dst = connect(dst_cfg, "目标").await?;

    apply_session_opts(&mut src, opts, false).await?;
    apply_session_opts(&mut dst, opts, true).await?;

    let dst_ver = introspect::server_version(&mut dst).await?;
    let dst_is_mariadb = introspect::is_mariadb(&mut dst).await?;
    let packet = introspect::max_allowed_packet(&mut dst).await?;
    let budget = (packet as f64 * PACKET_BUDGET_RATIO) as u64;

    let cursor = match opts.cursor_start {
        CursorStart::Full => Cursor::start(),
        CursorStart::Resume => {
            let max = read_max_pk(&mut dst, &pf.target_schema, &pf.target_table, &pk).await?;
            Cursor::from_max_pk(max)
        }
        CursorStart::Custom => match &opts.start_id {
            Some(s) if !s.trim().is_empty() => {
                Cursor(Some(parse_cursor_value(s, pf.primary_key_unsigned)?))
            }
            _ => Cursor::start(),
        },
    };

    let end_id = match &opts.end_id {
        Some(s) if !s.trim().is_empty() => {
            let v = parse_cursor_value(s, pf.primary_key_unsigned)?;
            Some(value::value_as_i128(&v).ok_or_else(|| {
                Error::Config(format!("结束游标 `{s}` 不是合法整数"))
            })?)
        }
        _ => None,
    };

    let pk_index = pf
        .columns
        .iter()
        .position(|c| c == &pk)
        .ok_or_else(|| Error::Other(format!("主键 {pk} 不在同步列中")))?;

    let mut run = TableRun {
        pk,
        pk_index,
        columns: pf.columns.clone(),
        cursor,
        end_id,
        batch_size: opts.batch_size.clamp(MIN_BATCH, opts.max_batch_size.max(MIN_BATCH)),
        src_schema: pf.source_schema.clone(),
        dst_schema: pf.target_schema.clone(),
        src_table: pf.table.clone(),
        dst_table: pf.target_table.clone(),
    };

    if !run.cursor.is_start() {
        sink.emit(Event::Log {
            table: Some(pf.table.clone()),
            level: "info".into(),
            message: format!("起始游标 {} > {}", run.pk, run.cursor.display()),
        });
    }

    let use_new_upsert = !dst_is_mariadb && dst_ver >= (8, 0, 19);
    // 占位符上限：行数 × 列数 ≤ 65535
    let placeholder_cap = (MAX_PLACEHOLDERS / run.columns.len().max(1)).max(1);

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let start = Instant::now();
        // 记录本批实际使用的批大小。自适应会改动 run.batch_size，
        // 若用改动后的值判断「是否已读完」，批大小翻倍时会误判提前结束而静默丢行。
        let fetch_size = run.batch_size;
        let (rows, wire_bytes) = fetch_batch(&mut src, &run).await?;
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
                "主键 {} 出现 NULL，无法用游标推进",
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
                    "本批约 {:.1}MB 超过包预算 {:.1}MB，批大小降至 {}",
                    wire_bytes as f64 / 1e6,
                    budget as f64 / 1e6,
                    run.batch_size
                ),
            });
            continue;
        }

        let write_result = write_batch(&mut dst, &run, &rows, opts, use_new_upsert).await;

        match write_result {
            Ok(()) => {}
            Err(e) if e.should_shrink_batch() => {
                if returned <= 1 || run.batch_size <= MIN_BATCH {
                    return Err(e);
                }
                run.batch_size = (run.batch_size / 2).max(MIN_BATCH);
                sink.emit(Event::Warning {
                    table: Some(pf.table.clone()),
                    message: format!("{}，批大小降至 {}", e, run.batch_size),
                });
                continue;
            }
            Err(e) => return Err(e),
        }

        run.cursor.advance(last_pk);
        stats.rows += returned as u64;
        stats.batches += 1;
        stats.cursor = run.cursor.display();

        let elapsed = start.elapsed();
        let rps = if elapsed.as_secs_f64() > 0.0 {
            returned as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        if opts.adaptive_batch {
            let ms = elapsed.as_millis() as u64;
            let cap = opts.max_batch_size.min(placeholder_cap);
            if ms < 200 {
                run.batch_size = (run.batch_size * 2).min(cap);
            } else if ms > 2000 {
                run.batch_size = (run.batch_size / 2).max(MIN_BATCH);
            }
        }

        sink.emit(Event::BatchDone {
            table: pf.table.clone(),
            rows: returned as u64,
            total_rows: stats.rows,
            cursor: run.cursor.display(),
            rows_per_sec: rps,
            batch_size: run.batch_size,
            elapsed_ms: elapsed.as_millis() as u64,
        });

        if let (Some(end), Some(cur)) = (run.end_id, run.cursor.as_i128()) {
            if cur >= end {
                sink.emit(Event::Log {
                    table: Some(pf.table.clone()),
                    level: "info".into(),
                    message: format!("已到达结束游标 {end}"),
                });
                break;
            }
        }

        if is_last_batch(returned, fetch_size) {
            // 本批没取满，说明源表已读完。必须用 fetch_size 而非当前的 run.batch_size：
            // 后面自适应可能刚把 batch_size 放大，拿放大后的值比较会导致误判提前结束。
            break;
        }
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

/// 读取一批数据。返回 (行, 估算线上字节数)。
async fn fetch_batch(src: &mut Conn, run: &TableRun) -> Result<(Vec<Vec<Value>>, u64)> {
    let cols = run
        .columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");

    let (sql, params) = if run.cursor.is_start() {
        (
            format!(
                "SELECT {cols} FROM {} ORDER BY {} ASC LIMIT ?",
                qualified(&run.src_schema, &run.src_table),
                quote_ident(&run.pk)
            ),
            Params::Positional(vec![Value::UInt(run.batch_size as u64)]),
        )
    } else {
        (
            format!(
                "SELECT {cols} FROM {} WHERE {} > ? ORDER BY {} ASC LIMIT ?",
                qualified(&run.src_schema, &run.src_table),
                quote_ident(&run.pk),
                quote_ident(&run.pk)
            ),
            Params::Positional(vec![
                run.cursor.0.clone().expect("非起始游标"),
                Value::UInt(run.batch_size as u64),
            ]),
        )
    };

    let mut result = src.exec_iter(sql, params).await?;
    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(run.batch_size.min(4096));
    let mut bytes: u64 = 0;

    while let Some(row) = result.next().await? {
        let values = row.unwrap();
        for v in &values {
            bytes += value::value_wire_size(v) as u64;
        }
        rows.push(values);
    }
    result.drop_result().await?;

    Ok((rows, bytes))
}

/// 写入一批。一个事务，一条多行 INSERT。
async fn write_batch(
    dst: &mut Conn,
    run: &TableRun,
    rows: &[Vec<Value>],
    opts: &TaskOptions,
    use_new_upsert: bool,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let ncols = run.columns.len();
    let cols = run
        .columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");
    let row_placeholder = format!(
        "({})",
        std::iter::repeat("?").take(ncols).collect::<Vec<_>>().join(",")
    );

    let mut params: Vec<Value> = Vec::with_capacity(rows.len() * ncols);
    for r in rows {
        debug_assert_eq!(r.len(), ncols, "列数不一致");
        params.extend(r.iter().cloned());
    }

    let values_clause = std::iter::repeat(row_placeholder.as_str())
        .take(rows.len())
        .collect::<Vec<_>>()
        .join(",");

    let sql = match opts.write_mode {
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
    };

    dst.query_drop("START TRANSACTION").await?;
    match dst.exec_drop(sql, Params::Positional(params)).await {
        Ok(()) => {
            dst.query_drop("COMMIT").await?;
            Ok(())
        }
        Err(e) => {
            let _ = dst.query_drop("ROLLBACK").await;
            Err(e.into())
        }
    }
}

async fn apply_session_opts(conn: &mut Conn, opts: &TaskOptions, is_target: bool) -> Result<()> {
    // 首版只在目标端放开这些开关：源端应保持只读的默认行为。
    if !is_target {
        return Ok(());
    }
    if opts.disable_foreign_key_checks {
        conn.query_drop("SET SESSION foreign_key_checks = 0").await?;
    }
    if opts.relax_sql_mode {
        conn.query_drop("SET SESSION sql_mode = ''").await?;
    }
    // 大字段传输时避免超时
    conn.query_drop("SET SESSION net_write_timeout = 600").await?;
    conn.query_drop("SET SESSION net_read_timeout = 600").await?;
    Ok(())
}

async fn read_max_pk(
    conn: &mut Conn,
    schema: &str,
    table: &str,
    pk: &str,
) -> Result<Option<Value>> {
    let sql = format!(
        "SELECT MAX({}) FROM {}",
        quote_ident(pk),
        qualified(schema, table)
    );
    let row: Option<(Option<Value>,)> = conn.query_first(sql).await?;
    Ok(row.and_then(|(v,)| v))
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
}
