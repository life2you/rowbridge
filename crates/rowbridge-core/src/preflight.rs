//! 结构预检。
//!
//! 在真正写数据之前，把所有可能导致失败或数据丢失的结构问题查清楚，
//! 以「检查清单」的形式返回给 UI。这是决定这个工具能否放心挨着生产库跑的关键。

use serde::{Deserialize, Serialize};

use crate::config::TableTaskConfig;
use crate::ddl::{rewrite_create_table, RewriteOptions, RewriteResult};
use crate::introspect::{self, TableDetail};
use crate::value::qualified;
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// 阻塞：不允许启动
    Error,
    /// 警告：需用户确认
    Warning,
    /// 信息
    Info,
    /// 通过
    Ok,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

impl Check {
    fn error(code: &str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Error, code: code.into(), message: message.into() }
    }
    fn warn(code: &str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Warning, code: code.into(), message: message.into() }
    }
    fn info(code: &str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Info, code: code.into(), message: message.into() }
    }
    fn ok(code: &str, message: impl Into<String>) -> Self {
        Self { severity: Severity::Ok, code: code.into(), message: message.into() }
    }
}

/// 单张表的预检结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePreflight {
    pub source_schema: String,
    pub table: String,
    pub target_schema: String,
    pub target_table: String,
    /// 能否启动
    pub syncable: bool,
    pub primary_key: Option<String>,
    pub primary_key_unsigned: bool,
    pub columns: Vec<String>,
    pub checks: Vec<Check>,
    /// 目标表是否需要新建
    pub needs_create_table: bool,
    /// 建表 DDL（含改写 diff）
    pub create_table: Option<RewriteResult>,
    /// 源表 MAX(pk) / 目标表 MAX(pk)，用于展示进度基准
    pub source_max_pk: Option<String>,
    pub target_max_pk: Option<String>,
    pub source_rows_estimate: u64,
    /// 本次同步预计处理的行数（综合考量自定义游标区间、断点续传已有最大主键与源表行数估算）
    pub planned_rows: u64,
    pub target_rows_exists: bool,
    /// 目标表缺失列时生成的 ALTER TABLE 语句列表（用于先同步结构）
    #[serde(default)]
    pub alter_sqls: Vec<String>,
    /// 本表生效的游标模式（表级优先，其次全局）
    pub cursor_start: crate::config::CursorStart,
    /// 本表生效的起始 id
    pub start_id: Option<String>,
    /// 本表生效的结束 id
    pub end_id: Option<String>,
}

impl TablePreflight {
    pub fn has_blocking_error(&self) -> bool {
        self.checks.iter().any(|c| c.severity == Severity::Error)
    }
}

/// 整个任务的预检结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub tables: Vec<TablePreflight>,
    pub source_version: String,
    pub target_version: String,
    /// 源与目标是同一个连接（同实例跨库）
    pub same_instance: bool,
    pub checks: Vec<Check>,
}

impl PreflightReport {
    pub fn any_blocking_error(&self) -> bool {
        self.checks.iter().any(|c| c.severity == Severity::Error)
            || self.tables.iter().any(|t| t.has_blocking_error())
    }

    pub fn syncable_tables(&self) -> Vec<&TablePreflight> {
        self.tables.iter().filter(|t| t.syncable).collect()
    }

    /// 是否存在需要同步的表结构（包含缺失表新建或已有表补全列）
    pub fn has_schema_changes(&self) -> bool {
        self.tables.iter().any(|t| t.needs_create_table || !t.alter_sqls.is_empty())
    }
}

fn version_string(v: (u16, u16, u16)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// 判断两套连接配置是否指向同一实例（用于「同实例跨库」提示）。
pub fn same_instance(a: &crate::config::ConnConfig, b: &crate::config::ConnConfig) -> bool {
    a.host.eq_ignore_ascii_case(&b.host) && a.port == b.port && a.username == b.username
}

/// 执行预检。
pub async fn run(
    src: &mut mysql_async::Conn,
    dst: &mut mysql_async::Conn,
    tasks: &[TableTaskConfig],
    options: &crate::config::TaskOptions,
    same_conn: bool,
) -> Result<PreflightReport> {
    let src_ver = introspect::server_version(src).await?;
    let dst_ver = introspect::server_version(dst).await?;
    let dst_is_mariadb = introspect::is_mariadb(dst).await?;

    let mut checks = Vec::new();

    if src_ver.0 == 0 {
        checks.push(Check::warn(
            "version_unknown",
            "无法读取源端版本号，跨版本建表可能失败",
        ));
    }
    if same_conn {
        checks.push(Check::info(
            "same_instance",
            "源与目标为同一实例，将走同实例跨库同步",
        ));
    }

    let mut tables = Vec::with_capacity(tasks.len());
    for task in tasks {
        tables.push(check_table(src, dst, task, options, dst_ver, dst_is_mariadb).await?);
    }

    Ok(PreflightReport {
        tables,
        source_version: version_string(src_ver),
        target_version: version_string(dst_ver),
        same_instance: same_conn,
        checks,
    })
}

async fn check_table(
    src: &mut mysql_async::Conn,
    dst: &mut mysql_async::Conn,
    task: &TableTaskConfig,
    options: &crate::config::TaskOptions,
    dst_ver: (u16, u16, u16),
    dst_is_mariadb: bool,
) -> Result<TablePreflight> {
    let mut checks = Vec::new();
    let dst_table_name = task.dst_table_name().to_string();

    // ---- 源表 ----
    let src_detail = introspect::table_detail(src, &task.src_schema, &task.table).await?;
    let src_rows_estimate = introspect::estimate_rows(src, &task.src_schema, &task.table).await?;

    // ---- 目标库 ----
    let dst_schema_exists = introspect::schema_exists(dst, &task.dst_schema).await?;
    if !dst_schema_exists {
        if options.create_database {
            checks.push(Check::ok(
                "target_schema_missing",
                format!("目标库 {} 不存在，将自动创建", task.dst_schema),
            ));
        } else {
            checks.push(Check::error(
                "target_schema_missing",
                format!("目标库 {} 不存在，且未开启自动建库", task.dst_schema),
            ));
        }
    }

    // ---- 目标表 ----
    let target_rows_exists = dst_schema_exists
        && introspect::table_exists(dst, &task.dst_schema, &dst_table_name).await?;

    let mut needs_create_table: bool = false;
    let mut create_table: Option<RewriteResult> = None;

    if !target_rows_exists {
        needs_create_table = true;
        if options.create_table {
            let ddl = introspect::show_create_table(src, &task.src_schema, &task.table).await?;
            let rewritten = rewrite_create_table(
                &ddl,
                &RewriteOptions {
                    dst_schema: task.dst_schema.clone(),
                    dst_table: dst_table_name.clone(),
                    strip_auto_increment: options.strip_auto_increment,
                    strip_foreign_keys: options.strip_foreign_keys,
                    map_collation: options.map_collation,
                    target_version: dst_ver,
                    target_is_mariadb: dst_is_mariadb,
                },
            );
            for w in &rewritten.warnings {
                checks.push(Check::warn("ddl_compat", w.clone()));
            }
            checks.push(Check::ok(
                "need_create",
                format!(
                    "目标表 {}.{} 不存在，将按源表结构自动创建（{} 处改写）",
                    task.dst_schema,
                    dst_table_name,
                    rewritten.changes.len()
                ),
            ));
            create_table = Some(rewritten);
        } else {
            checks.push(Check::error(
                "target_missing",
                format!(
                    "目标表 {}.{} 不存在，且未开启自动建表",
                    task.dst_schema, dst_table_name
                ),
            ));
        }
    }

    // ---- 主键 ----
    let (pk_name, pk_unsigned, syncable_pk) = match src_detail.pk_column() {
        Ok(col) => {
            let unsigned = col.column_type.to_ascii_lowercase().contains("unsigned");
            checks.push(Check::ok(
                "pk_ok",
                format!(
                    "主键 {} ({}) 为单列整型，符合分批同步条件",
                    col.name, col.column_type
                ),
            ));
            if col.extra.to_ascii_lowercase().contains("auto_increment") {
                checks.push(Check::info(
                    "pk_auto_increment",
                    "主键为 AUTO_INCREMENT，值会原样写入目标表",
                ));
            }
            (Some(col.name.clone()), unsigned, true)
        }
        Err(e) => {
            checks.push(Check::error("pk_unsupported", e.to_string()));
            (src_detail.primary_key.first().cloned(), false, false)
        }
    };

    // ---- 列兼容性 ----
    let src_cols = src_detail.copyable_columns();
    let columns: Vec<String> = if task.columns.is_empty() {
        src_cols.iter().map(|c| c.name.clone()).collect()
    } else {
        let known: Vec<&str> = src_cols.iter().map(|c| c.name.as_str()).collect();
        let missing: Vec<&String> = task
            .columns
            .iter()
            .filter(|c| !known.contains(&c.as_str()))
            .collect();
        if !missing.is_empty() {
            checks.push(Check::error(
                "column_missing_in_source",
                format!("源表不存在这些列（或为生成列）: {missing:?}"),
            ));
        }
        task.columns.clone()
    };

    if src_cols.len() != src_detail.columns.len() {
        let generated: Vec<&str> = src_detail
            .columns
            .iter()
            .filter(|c| {
                let e = c.extra.to_ascii_lowercase();
                e.contains("virtual generated") || e.contains("stored generated")
            })
            .map(|c| c.name.as_str())
            .collect();
        if !generated.is_empty() {
            checks.push(Check::info(
                "generated_columns",
                format!("跳过生成列（不可显式写入）: {generated:?}"),
            ));
        }
    }

    if columns.is_empty() {
        checks.push(Check::error("no_columns", "没有可同步的列"));
    }

    // ---- 与目标表已有结构的差异 ----
    let mut target_max_pk = None;
    let mut alter_sqls = Vec::new();
    if target_rows_exists {
        let dst_detail = introspect::table_detail(dst, &task.dst_schema, &dst_table_name).await?;
        compare_columns(
            &src_detail,
            &dst_detail,
            &columns,
            &task.dst_schema,
            &dst_table_name,
            dst_ver,
            &mut checks,
            &mut alter_sqls,
        );

        // 目标端多出的 NOT NULL 无默认值列会导致写入失败
        let missing_required: Vec<String> = dst_detail
            .columns
            .iter()
            .filter(|c| !src_detail.columns.iter().any(|s| s.name == c.name))
            .filter(|c| !c.is_nullable && c.column_default.is_none())
            .filter(|c| !c.extra.to_ascii_lowercase().contains("auto_increment"))
            .map(|c| c.name.clone())
            .collect();
        if !missing_required.is_empty() {
            checks.push(Check::error(
                "target_required_columns",
                format!(
                    "目标表存在源表没有的 NOT NULL 且无默认值的列，写入必定失败: {missing_required:?}。请先补列或改为可空。"
                ),
            ));
        }

        if !dst_detail.triggers.is_empty() {
            checks.push(Check::warn(
                "target_triggers",
                format!(
                    "目标表存在触发器 {:?}，可能改变写入结果或导致行数不符",
                    dst_detail.triggers
                ),
            ));
        }

        if let Some(pk) = &pk_name {
            target_max_pk = read_max_pk(dst, &task.dst_schema, &dst_table_name, pk).await?;
        }
    }

    let source_max_pk = if let Some(pk) = &pk_name {
        read_max_pk(src, &task.src_schema, &task.table, pk).await?
    } else {
        None
    };

    if !options.strip_foreign_keys && needs_create_table && syncable_pk {
        checks.push(Check::warn(
            "fk_kept",
            "建表时保留了外键约束，若被引用表尚未创建会建表失败",
        ));
    }

    if let Some(pk) = &pk_name {
        if let Some(src_col) = src_detail.columns.iter().find(|c| &c.name == pk) {
            check_pk_width(src_col, &mut checks);
        }
    }

    if options.max_retries == 0 {
        checks.push(Check::warn(
            "no_retry",
            "未开启失败重试，网络抖动会直接导致任务失败",
        ));
    }

    let syncable = syncable_pk
        && !checks.iter().any(|c| c.severity == Severity::Error)
        && !columns.is_empty();

    // 不可同步的表不展示建表方案，避免误导用户以为它会被创建。
    if !syncable {
        create_table = None;
    }

    let cursor_start = task.effective_cursor_start(options);
    let will_truncate = task.effective_truncate_target(options);
    if will_truncate && target_rows_exists {
        checks.push(Check::warn(
            "will_truncate_target",
            format!(
                "⚠️ 已开启清空目标表：同步前将执行 TRUNCATE TABLE 清空目标表 {}.{} 的已有数据",
                task.dst_schema, dst_table_name
            ),
        ));
    }

    let start_id = task
        .effective_start_id(options)
        .filter(|s| !s.trim().is_empty());
    let end_id = task
        .effective_end_id(options)
        .filter(|s| !s.trim().is_empty());

    // 自定义游标填错了会静默同步 0 行，这里提前点破。
    if cursor_start == crate::config::CursorStart::Custom {
        match &start_id {
            None => checks.push(Check::warn(
                "cursor_custom_empty",
                "选了自定义游标但没填起始 id，将等同全量",
            )),
            Some(v) => {
                if let Ok(maxv) = v.parse::<i128>() {
                    if let Some(src_max) = source_max_pk.as_ref().and_then(|m| m.parse::<i128>().ok()) {
                        if maxv > src_max {
                            checks.push(Check::error(
                                "cursor_beyond_max",
                                format!(
                                    "起始 id {maxv} 大于源表 MAX(pk) {src_max}，本表将同步 0 行"
                                ),
                            ));
                        }
                    }
                    if let Some(endv) = end_id.as_ref().and_then(|e| e.parse::<i128>().ok()) {
                        if endv < maxv {
                            checks.push(Check::error(
                                "cursor_empty_range",
                                format!("结束 id {endv} 小于起始 id {maxv}，区间为空"),
                            ));
                        }
                    }
                }
            }
        }
    }

    let planned_rows = estimate_planned_rows(
        src_rows_estimate,
        source_max_pk.as_deref(),
        if will_truncate { None } else { target_max_pk.as_deref() },
        cursor_start,
        start_id.as_deref(),
        end_id.as_deref(),
    );

    Ok(TablePreflight {
        source_schema: task.src_schema.clone(),
        table: task.table.clone(),
        target_schema: task.dst_schema.clone(),
        target_table: dst_table_name,
        syncable,
        primary_key: pk_name,
        primary_key_unsigned: pk_unsigned,
        columns,
        checks,
        needs_create_table,
        create_table,
        source_max_pk,
        target_max_pk,
        source_rows_estimate: src_rows_estimate,
        planned_rows,
        target_rows_exists,
        alter_sqls,
        cursor_start,
        start_id,
        end_id,
    })
}

/// 估算在特定游标模式下本次任务预期同步的行数。
pub fn estimate_planned_rows(
    source_total_estimate: u64,
    source_max_pk: Option<&str>,
    target_max_pk: Option<&str>,
    cursor_start: crate::config::CursorStart,
    start_id: Option<&str>,
    end_id: Option<&str>,
) -> u64 {
    let src_max = source_max_pk.and_then(|s| s.parse::<i128>().ok());
    let dst_max = target_max_pk.and_then(|s| s.parse::<i128>().ok());
    let start_num = start_id.and_then(|s| s.parse::<i128>().ok());
    let end_num = end_id.and_then(|s| s.parse::<i128>().ok());

    match cursor_start {
        crate::config::CursorStart::Custom => {
            let start = start_num.unwrap_or(0);
            let end = end_num.or(src_max);
            match end {
                Some(end_val) => {
                    if end_val < start {
                        0
                    } else {
                        let span = (end_val - start + 1) as u64;
                        if source_total_estimate > 0 {
                            span.min(source_total_estimate)
                        } else {
                            span
                        }
                    }
                }
                None => source_total_estimate,
            }
        }
        crate::config::CursorStart::Resume => {
            let start = dst_max.unwrap_or(0);
            let end = end_num.or(src_max);
            match end {
                Some(end_val) => {
                    if end_val <= start {
                        0
                    } else {
                        let span = (end_val - start) as u64;
                        if source_total_estimate > 0 {
                            span.min(source_total_estimate)
                        } else {
                            span
                        }
                    }
                }
                None => source_total_estimate,
            }
        }
        crate::config::CursorStart::Full => {
            if let Some(end_val) = end_num {
                if end_val <= 0 {
                    0
                } else if source_total_estimate > 0 {
                    (end_val as u64).min(source_total_estimate)
                } else {
                    end_val as u64
                }
            } else {
                source_total_estimate
            }
        }
    }
}

fn check_pk_width(src_col: &crate::introspect::ColumnInfo, checks: &mut Vec<Check>) {
    let ct = src_col.column_type.to_ascii_lowercase();
    if ct.contains("unsigned") && ct.starts_with("bigint") {
        checks.push(Check::info(
            "pk_unsigned_bigint",
            "主键为 bigint unsigned，游标以原始值传递，可支持到 u64::MAX",
        ));
    }
}

/// 生成向目标表追加新列的 `ALTER TABLE` 语句。
pub fn build_add_column_ddl(
    dst_schema: &str,
    dst_table: &str,
    col: &crate::introspect::ColumnInfo,
    after_col: Option<&str>,
    target_version: (u16, u16, u16),
) -> String {
    let mut sql = format!(
        "ALTER TABLE {}.{} ADD COLUMN {} {}",
        crate::value::quote_ident(dst_schema),
        crate::value::quote_ident(dst_table),
        crate::value::quote_ident(&col.name),
        col.column_type
    );

    let mut collation = col.collation.clone();
    if target_version.0 > 0 && target_version < (8, 0, 0) {
        if let Some(c) = &collation {
            if c.contains("0900") {
                collation = Some("utf8mb4_general_ci".to_string());
            }
        }
    }

    if let Some(cs) = &col.charset {
        if !cs.is_empty() && (col.data_type.contains("char") || col.data_type.contains("text")) {
            sql.push_str(&format!(" CHARACTER SET {cs}"));
        }
    }
    if let Some(coll) = &collation {
        if !coll.is_empty() && (col.data_type.contains("char") || col.data_type.contains("text")) {
            sql.push_str(&format!(" COLLATE {coll}"));
        }
    }

    if col.is_nullable {
        sql.push_str(" NULL");
    } else {
        sql.push_str(" NOT NULL");
    }

    if let Some(def) = &col.column_default {
        let trimmed = def.trim();
        let upper = trimmed.to_ascii_uppercase();
        if upper == "NULL" {
            sql.push_str(" DEFAULT NULL");
        } else if upper.starts_with("CURRENT_TIMESTAMP") || upper == "NOW()" {
            sql.push_str(&format!(" DEFAULT {trimmed}"));
        } else if trimmed.starts_with('\'') || trimmed.parse::<f64>().is_ok() {
            sql.push_str(&format!(" DEFAULT {trimmed}"));
        } else {
            sql.push_str(&format!(" DEFAULT '{}'", trimmed.replace('\'', "''")));
        }
    }

    let extra_lower = col.extra.to_ascii_lowercase();
    if extra_lower.contains("on update current_timestamp") {
        sql.push_str(" ON UPDATE CURRENT_TIMESTAMP");
    }

    if let Some(after) = after_col {
        sql.push_str(&format!(" AFTER {}", crate::value::quote_ident(after)));
    }

    sql
}

fn compare_columns(
    src: &TableDetail,
    dst: &TableDetail,
    used: &[String],
    dst_schema: &str,
    dst_table: &str,
    target_version: (u16, u16, u16),
    checks: &mut Vec<Check>,
    alter_sqls: &mut Vec<String>,
) {
    let mut narrowed = Vec::new();
    let mut type_changed = Vec::new();
    let mut missing_cols = Vec::new();

    for name in used {
        let Some(s) = src.columns.iter().find(|c| &c.name == name) else {
            continue;
        };
        let Some(d) = dst.columns.iter().find(|c| &c.name == name) else {
            let src_idx = src.columns.iter().position(|c| &c.name == name).unwrap_or(0);
            let after_col = if src_idx > 0 {
                src.columns[..src_idx]
                    .iter()
                    .rev()
                    .find(|c| dst.columns.iter().any(|dc| dc.name == c.name))
                    .map(|c| c.name.as_str())
            } else {
                None
            };
            alter_sqls.push(build_add_column_ddl(
                dst_schema,
                dst_table,
                s,
                after_col,
                target_version,
            ));
            missing_cols.push(name.clone());
            continue;
        };

        let s_ty = s.column_type.to_ascii_lowercase();
        let d_ty = d.column_type.to_ascii_lowercase();
        if s_ty == d_ty {
            continue;
        }

        let (s_base, s_len) = split_type(&s_ty);
        let (d_base, d_len) = split_type(&d_ty);
        if s_base != d_base {
            type_changed.push(format!("{}: {} → {}", name, s_ty, d_ty));
        } else if let (Some(sl), Some(dl)) = (s_len, d_len) {
            if sl > dl {
                narrowed.push(format!("{}: {} → {}", name, s_ty, d_ty));
            }
        }
    }

    if !missing_cols.is_empty() {
        checks.push(Check::warn(
            "column_missing_in_target",
            format!("目标表缺少列 {:?}，支持先同步表结构自动补全", missing_cols),
        ));
    }

    if !narrowed.is_empty() {
        checks.push(Check::error(
            "column_narrowed",
            format!("目标列长度小于源列，写入会报 1406 Data too long: {narrowed:?}"),
        ));
    }
    if !type_changed.is_empty() {
        checks.push(Check::warn(
            "column_type_changed",
            format!("以下列类型与源表不同，可能被隐式转换: {type_changed:?}"),
        ));
    }

    let src_coll = src.columns.iter().find_map(|c| c.collation.clone());
    let dst_coll = dst.columns.iter().find_map(|c| c.collation.clone());
    if let (Some(a), Some(b)) = (src_coll, dst_coll) {
        if a != b {
            checks.push(Check::warn(
                "collation_mismatch",
                format!("源表 collation 为 {a}，目标表为 {b}，跨字符集写入可能报 1366"),
            ));
        }
    }
}

/// 从 `varchar(64)` 里拆出 (`varchar`, Some(64))。无长度返回 None。
fn split_type(ty: &str) -> (String, Option<u64>) {
    let ty = ty.split_whitespace().next().unwrap_or(ty);
    match ty.find('(') {
        Some(i) => {
            let base = ty[..i].to_string();
            let inner = &ty[i + 1..];
            let num = inner
                .split(')')
                .next()
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse::<u64>().ok());
            (base, num)
        }
        None => (ty.to_string(), None),
    }
}

async fn read_max_pk(
    conn: &mut mysql_async::Conn,
    schema: &str,
    table: &str,
    pk: &str,
) -> Result<Option<String>> {
    use mysql_async::prelude::*;
    let sql = format!(
        "SELECT MAX({}) FROM {}",
        crate::value::quote_ident(pk),
        qualified(schema, table)
    );
    let row: Option<(Option<mysql_async::Value>,)> = conn.query_first(sql).await?;
    Ok(match row.and_then(|(v,)| v) {
        None | Some(mysql_async::Value::NULL) => None,
        Some(v) => Some(crate::value::value_to_display(&v)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_planned_rows_for_custom_range() {
        // 模拟用户截图场景：总行数 66,381,892，源 MAX 185,828,688，区间 185192703..185741315
        // 闭区间跨度就是 185741315 - 185192703 + 1 = 548,613 行，实际同步恰为 548,613 行
        let est = estimate_planned_rows(
            66_381_892,
            Some("185828688"),
            None,
            crate::config::CursorStart::Custom,
            Some("185192703"),
            Some("185741315"),
        );
        let span = 185_741_315 - 185_192_703 + 1;
        assert_eq!(est, span);

        // 没有 MAX(pk) 时，闭区间 [100, 600] 包含 100 和 600，跨度为 501
        let est_no_max = estimate_planned_rows(
            65_234_453,
            None,
            None,
            crate::config::CursorStart::Custom,
            Some("100"),
            Some("600"),
        );
        assert_eq!(est_no_max, 501);

        // 单点闭区间 [500, 500] 包含 1 行
        let est_single = estimate_planned_rows(
            1000,
            None,
            None,
            crate::config::CursorStart::Custom,
            Some("500"),
            Some("500"),
        );
        assert_eq!(est_single, 1);

        // 结束 id < 起始 id 时为 0
        let est_zero = estimate_planned_rows(
            1000,
            None,
            None,
            crate::config::CursorStart::Custom,
            Some("500"),
            Some("499"),
        );
        assert_eq!(est_zero, 0);
    }

    #[test]
    fn estimates_planned_rows_for_resume() {
        // 续传：源 MAX 1000，目标已到 800
        let est = estimate_planned_rows(
            1000,
            Some("1000"),
            Some("800"),
            crate::config::CursorStart::Resume,
            None,
            None,
        );
        assert_eq!(est, 200);

        // 目标已经跟上源端（甚至大于源端），剩余为 0
        let est_done = estimate_planned_rows(
            1000,
            Some("1000"),
            Some("1000"),
            crate::config::CursorStart::Resume,
            None,
            None,
        );
        assert_eq!(est_done, 0);
    }

    #[test]
    fn splits_types() {
        assert_eq!(split_type("varchar(64)"), ("varchar".into(), Some(64)));
        assert_eq!(split_type("int"), ("int".into(), None));
        assert_eq!(split_type("decimal(10,2)"), ("decimal".into(), Some(10)));
        assert_eq!(split_type("bigint unsigned"), ("bigint".into(), None));
    }

    #[test]
    fn detects_narrowed_column() {
        use crate::introspect::{ColumnInfo, IndexInfo};
        let mk = |name: &str, ct: &str| ColumnInfo {
            name: name.into(),
            data_type: ct.split('(').next().unwrap().into(),
            column_type: ct.into(),
            is_nullable: true,
            column_default: None,
            extra: String::new(),
            charset: None,
            collation: None,
            is_primary: false,
        };
        let src = TableDetail {
            schema: "s".into(),
            table: "t".into(),
            columns: vec![mk("name", "varchar(128)")],
            indexes: Vec::<IndexInfo>::new(),
            primary_key: vec!["id".into()],
            triggers: vec![],
        };
        let dst = TableDetail {
            schema: "d".into(),
            table: "t".into(),
            columns: vec![mk("name", "varchar(32)")],
            indexes: Vec::<IndexInfo>::new(),
            primary_key: vec!["id".into()],
            triggers: vec![],
        };
        let mut checks = Vec::new();
        let mut alter_sqls = Vec::new();
        compare_columns(
            &src,
            &dst,
            &["name".to_string()],
            "d",
            "t",
            (8, 0, 36),
            &mut checks,
            &mut alter_sqls,
        );
        assert!(checks.iter().any(|c| c.code == "column_narrowed"));
    }

    #[test]
    fn generates_alter_table_for_missing_column() {
        use crate::introspect::{ColumnInfo, IndexInfo};
        let mk = |name: &str, ct: &str| ColumnInfo {
            name: name.into(),
            data_type: ct.split('(').next().unwrap().into(),
            column_type: ct.into(),
            is_nullable: true,
            column_default: None,
            extra: String::new(),
            charset: None,
            collation: None,
            is_primary: false,
        };
        let src = TableDetail {
            schema: "s".into(),
            table: "t".into(),
            columns: vec![mk("id", "int"), mk("name", "varchar(64)"), mk("age", "int")],
            indexes: Vec::<IndexInfo>::new(),
            primary_key: vec!["id".into()],
            triggers: vec![],
        };
        let dst = TableDetail {
            schema: "d".into(),
            table: "t".into(),
            columns: vec![mk("id", "int"), mk("name", "varchar(64)")],
            indexes: Vec::<IndexInfo>::new(),
            primary_key: vec!["id".into()],
            triggers: vec![],
        };
        let mut checks = Vec::new();
        let mut alter_sqls = Vec::new();
        compare_columns(
            &src,
            &dst,
            &["id".into(), "name".into(), "age".into()],
            "d",
            "t",
            (8, 0, 36),
            &mut checks,
            &mut alter_sqls,
        );
        assert_eq!(alter_sqls.len(), 1);
        assert!(alter_sqls[0].contains("ADD COLUMN `age` int"));
        assert!(alter_sqls[0].contains("AFTER `name`"));
    }
}
