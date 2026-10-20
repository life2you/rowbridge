//! Tauri 命令层。
//!
//! 约定：所有命令都是 async，返回 `Result<T, String>`，前端拿到的是可直接展示的错误文案。
//! 长任务通过 `Channel<Event>` 推事件，用 `taskId` 关联取消令牌。

use std::collections::HashMap;
use std::sync::Arc;

use rowbridge_core::config::{
    ConnConfig, CursorStart, TableTaskConfig, TaskConfig, TaskOptions, WriteMode,
};
use rowbridge_core::engine::{self, Event, EventSink, TaskReport};
use rowbridge_core::introspect;
use rowbridge_core::preflight::{self, PreflightReport};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;


pub struct AppState {
    /// 正在运行的任务：taskId → 取消令牌
    pub tasks: Mutex<HashMap<String, CancellationToken>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
        }
    }
}

/// 前端传来的连接表单。
#[derive(Debug, Clone, Deserialize)]
pub struct ConnForm {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 为空时从 Keychain 读取（按 connection_id）
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub use_tls: bool,
}

impl ConnForm {
    fn resolve(&self) -> Result<ConnConfig, String> {
        let password = match &self.password {
            Some(p) if !p.is_empty() => p.clone(),
            _ => match &self.connection_id {
                // 密码在 ~/.RowBridge/connections.json 里，按 id 找
                Some(id) => crate::connections::list()
                    .ok()
                    .and_then(|l| l.into_iter().find(|c| c.id == *id))
                    .map(|c| c.password)
                    .unwrap_or_default(),
                None => String::new(),
            },
        };
        Ok(ConnConfig {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password,
            schema: None,
            use_tls: self.use_tls,
        })
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---------------------------------------------------------------- 连接

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub version: String,
    pub is_mariadb: bool,
    pub schemas: usize,
}

#[derive(Debug, Serialize)]
pub struct TableRow {
    pub name: String,
    pub rows_estimate: u64,
    pub engine: Option<String>,
    pub primary_key: Option<String>,
    pub unsupported_reason: Option<String>,
    pub is_view: bool,
    /// 主键最大值。懒加载：列表快路径不取，勾选时由 validate_tables 填。
    pub max_pk: Option<u64>,
}


#[tauri::command]
pub async fn test_connection(form: ConnForm) -> Result<TestResult, String> {
    let cfg = form.resolve()?;
    let mut conn = mysql_async::Conn::new(cfg.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    let version = introspect::server_version(&mut conn).await.map_err(err)?;
    let is_mariadb = introspect::is_mariadb(&mut conn).await.map_err(err)?;
    let schemas = introspect::list_schemas(&mut conn).await.map_err(err)?.len();
    let _ = conn.disconnect().await;
    Ok(TestResult {
        version: format!("{}.{}.{}", version.0, version.1, version.2),
        is_mariadb,
        schemas,
    })
}

#[tauri::command]
pub async fn list_schemas(form: ConnForm) -> Result<Vec<introspect::SchemaInfo>, String> {
    let cfg = form.resolve()?;
    let mut conn = mysql_async::Conn::new(cfg.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    let out = introspect::list_schemas(&mut conn).await.map_err(err)?;
    let _ = conn.disconnect().await;
    Ok(out)
}

#[tauri::command]
pub async fn list_tables(
    form: ConnForm,
    schema: String,
) -> Result<Vec<TableRow>, String> {
    let cfg = form.resolve()?;
    let mut conn = mysql_async::Conn::new(cfg.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接失败: {e}"))?;

    // 快路径：只做 2 个查询。
    // 1. TABLES 拿表清单
    // 2. 全库主键（一次批量），用于「不支持」置灰
    // 重活（COLUMNS/TRIGGERS/MAX(id)）交给 validate_tables，勾选时按需执行。
    let tables = introspect::list_tables(&mut conn, &schema).await.map_err(err)?;
    let pk_rows = introspect::list_primary_keys(&mut conn, &schema).await.map_err(err)?;

    // (表名 -> (按序主键列, 复合主键))
    let mut pk_map: std::collections::HashMap<String, (Vec<String>, bool)> =
        std::collections::HashMap::new();
    for (table, col, _seq, _non_unique) in pk_rows {
        let e = pk_map.entry(table).or_insert_with(|| (Vec::new(), false));
        e.0.push(col);
    }

    let mut out = Vec::with_capacity(tables.len());
    for t in tables {
        let mut primary_key: Option<String> = None;
        let mut unsupported_reason: Option<String> = None;
        if t.table_type.as_deref().map(|ty| ty.eq_ignore_ascii_case("VIEW")).unwrap_or(false) {
            unsupported_reason = Some("视图".into());
        } else if let Some((cols, _)) = pk_map.get(&t.name) {
            if cols.len() > 1 {
                unsupported_reason = Some(format!("复合主键 ({})", cols.join(", ")));
            } else if cols.len() == 1 {
                primary_key = Some(cols[0].clone());
                // 主键列类型是否为整型？信息不在 STATISTICS 里，需要 COLUMNS。
                // 快路径先不判——置灰只拦「复合/无主键」，
                // 整型校验留给勾选时的 validate_tables。
            }
        } else {
            unsupported_reason = Some("无主键".into());
        }

        out.push(TableRow {
            name: t.name,
            rows_estimate: t.rows_estimate.unwrap_or(0),
            engine: t.engine,
            primary_key,
            unsupported_reason,
            is_view: t.table_type.as_deref().map(|ty| ty.eq_ignore_ascii_case("VIEW")).unwrap_or(false),
            max_pk: None, // 懒加载：勾选时才取
        });
    }
    let _ = conn.disconnect().await;
    Ok(out)
}

/// 勾选表时按需校验（重活懒加载）。
///
/// 逐表做：COLUMNS（主键类型是否整型）、TRIGGERS、MAX(pk)。
/// 限流并发 ≤3，复用同一个连接，不再每表开新连接。
#[tauri::command]
pub async fn validate_tables(
    form: ConnForm,
    schema: String,
    tables: Vec<String>,
) -> Result<Vec<ValidatedTable>, String> {
    let cfg = form.resolve()?;
    let conn = mysql_async::Conn::new(cfg.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    let conn = std::sync::Arc::new(tokio::sync::Mutex::new(conn));
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));

    let mut futs = Vec::new();
    for t in tables {
        let conn = conn.clone();
        let sem = sem.clone();
        let schema = schema.clone();
        futs.push(tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return None,
            };
            let mut guard = conn.lock().await;
            validate_one(&mut guard, &schema, &t).await.ok()
        }));
    }

    let mut out = Vec::new();
    for f in futs {
        if let Ok(Some(v)) = f.await {
            out.push(v);
        }
    }
    // 保持入参顺序稳定
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct ValidatedTable {
    pub name: String,
    pub primary_key: Option<String>,
    pub primary_key_unsigned: bool,
    pub unsupported_reason: Option<String>,
    pub max_pk: Option<u64>,
    pub trigger_count: usize,
}

async fn validate_one(
    conn: &mut mysql_async::Conn,
    schema: &str,
    table: &str,
) -> Result<ValidatedTable, String> {
    let detail = introspect::table_detail(conn, schema, table).await.map_err(err)?;
    let (primary_key, primary_key_unsigned, unsupported_reason) = match detail.pk_column() {
        Ok(col) => (
            Some(col.name.clone()),
            col.column_type.to_ascii_lowercase().contains("unsigned"),
            None,
        ),
        Err(e) => (detail.primary_key.first().cloned(), false, Some(e.to_string())),
    };

    let max_pk = match &primary_key {
        Some(pk) if unsupported_reason.is_none() => {
            use mysql_async::prelude::*;
            let max: Option<u64> = conn
                .query_first(format!(
                    "SELECT MAX({}) FROM {}",
                    rowbridge_core::value::quote_ident(pk),
                    rowbridge_core::value::qualified(schema, table)
                ))
                .await
                .ok()
                .flatten()
                .flatten();
            max
        }
        _ => None,
    };

    Ok(ValidatedTable {
        name: table.to_string(),
        primary_key,
        primary_key_unsigned,
        unsupported_reason,
        max_pk,
        trigger_count: detail.triggers.len(),
    })
}

/// 读取表结构，用于详情面板。
#[tauri::command]
pub async fn describe_table(
    form: ConnForm,
    schema: String,
    table: String,
) -> Result<introspect::TableDetail, String> {
    let cfg = form.resolve()?;
    let mut conn = mysql_async::Conn::new(cfg.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    let out = introspect::table_detail(&mut conn, &schema, &table)
        .await
        .map_err(err)?;
    let _ = conn.disconnect().await;
    Ok(out)
}

// ---------------------------------------------------------------- 预检

#[derive(Debug, Clone, Deserialize)]
pub struct TableSpec {
    pub table: String,
    /// 目标表名，留空表示同名
    #[serde(default)]
    pub dst_table: Option<String>,
    /// 表级游标模式；不传则跟随全局
    #[serde(default)]
    pub cursor_start: Option<String>,
    #[serde(default)]
    pub start_id: Option<String>,
    #[serde(default)]
    pub end_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunRequest {
    pub source: ConnForm,
    pub target: ConnForm,
    pub src_schema: String,
    pub dst_schema: String,
    pub tables: Vec<TableSpec>,
    #[serde(default)]
    pub options: OptionsForm,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OptionsForm {
    #[serde(default = "d_parallel")]
    pub parallelism: usize,
    #[serde(default = "d_batch")]
    pub batch_size: usize,
    #[serde(default = "d_max_batch")]
    pub max_batch_size: usize,
    #[serde(default = "d_true")]
    pub adaptive_batch: bool,
    #[serde(default)]
    pub cursor_start: String,
    #[serde(default)]
    pub start_id: Option<String>,
    #[serde(default)]
    pub end_id: Option<String>,
    #[serde(default = "d_true")]
    pub create_table: bool,
    #[serde(default = "d_true")]
    pub create_database: bool,
    #[serde(default = "d_true")]
    pub strip_foreign_keys: bool,
    #[serde(default = "d_true")]
    pub strip_auto_increment: bool,
    #[serde(default = "d_true")]
    pub map_collation: bool,
    #[serde(default)]
    pub disable_foreign_key_checks: bool,
    #[serde(default)]
    pub relax_sql_mode: bool,
    #[serde(default = "d_true")]
    pub continue_on_error: bool,
}

fn d_parallel() -> usize {
    2
}
fn d_batch() -> usize {
    1000
}
fn d_max_batch() -> usize {
    10_000
}
fn d_true() -> bool {
    true
}

impl Default for OptionsForm {
    fn default() -> Self {
        Self {
            parallelism: d_parallel(),
            batch_size: d_batch(),
            max_batch_size: d_max_batch(),
            adaptive_batch: true,
            cursor_start: "full".into(),
            start_id: None,
            end_id: None,
            create_table: true,
            create_database: true,
            strip_foreign_keys: true,
            strip_auto_increment: true,
            map_collation: true,
            disable_foreign_key_checks: false,
            relax_sql_mode: false,
            continue_on_error: true,
        }
    }
}

fn to_task_config(req: &RunRequest) -> Result<TaskConfig, String> {
    let source = req.source.resolve()?;
    let target = req.target.resolve()?;
    let o = &req.options;
    let options = TaskOptions {
        parallelism: o.parallelism.max(1),
        batch_size: o.batch_size.max(1),
        max_batch_size: o.max_batch_size.max(o.batch_size.max(1)),
        adaptive_batch: o.adaptive_batch,
        cursor_start: match o.cursor_start.as_str() {
            "resume" => CursorStart::Resume,
            "custom" => CursorStart::Custom,
            _ => CursorStart::Full,
        },
        write_mode: WriteMode::Upsert,
        end_id: o.end_id.clone().filter(|s| !s.trim().is_empty()),
        start_id: o.start_id.clone().filter(|s| !s.trim().is_empty()),
        create_table: o.create_table,
        create_database: o.create_database,
        strip_foreign_keys: o.strip_foreign_keys,
        strip_auto_increment: o.strip_auto_increment,
        map_collation: o.map_collation,
        disable_foreign_key_checks: o.disable_foreign_key_checks,
        relax_sql_mode: o.relax_sql_mode,
        continue_on_error: o.continue_on_error,
        max_retries: 3,
    };
    let tables = req
        .tables
        .iter()
        .map(|t| TableTaskConfig {
            table: t.table.clone(),
            src_schema: req.src_schema.clone(),
            dst_schema: req.dst_schema.clone(),
            dst_table: t.dst_table.clone().filter(|s| !s.is_empty()),
            columns: Vec::new(),
            cursor_start: t.cursor_start.as_deref().and_then(|s| match s {
                "full" => Some(CursorStart::Full),
                "resume" => Some(CursorStart::Resume),
                "custom" => Some(CursorStart::Custom),
                _ => None,
            }),
            start_id: t.start_id.clone(),
            end_id: t.end_id.clone(),
        })
        .collect();
    Ok(TaskConfig {
        source,
        target,
        tables,
        options,
    })
}

#[tauri::command]
pub async fn preflight(req: RunRequest) -> Result<PreflightReport, String> {
    let cfg = to_task_config(&req)?;
    let same = preflight::same_instance(&cfg.source, &cfg.target);
    let mut sc = mysql_async::Conn::new(cfg.source.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接源库失败: {e}"))?;
    let mut dc = mysql_async::Conn::new(cfg.target.to_url().map_err(err)?)
        .await
        .map_err(|e| format!("连接目标库失败: {e}"))?;
    let report = preflight::run(&mut sc, &mut dc, &cfg.tables, &cfg.options, same)
        .await
        .map_err(err)?;
    let _ = sc.disconnect().await;
    let _ = dc.disconnect().await;
    Ok(report)
}

// ---------------------------------------------------------------- 执行

#[tauri::command]
pub async fn start_sync(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    req: RunRequest,
    task_id: String,
    report: PreflightReport,
    on_event: Channel<Event>,
) -> Result<TaskReport, String> {
    let cfg = to_task_config(&req)?;

    let cancel = CancellationToken::new();
    {
        let mut tasks = state.tasks.lock().await;
        if tasks.contains_key(&task_id) {
            return Err("任务已存在".into());
        }
        tasks.insert(task_id.clone(), cancel.clone());
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let sink = EventSink::new(tx);

    // 先落一条 running 记录，任务异常退出时历史里也能看到痕迹
    let _ = crate::store::begin_task(
        &app,
        &task_id,
        &format!("{}:{}/{}", req.source.host, req.source.port, req.src_schema),
        &format!("{}:{}/{}", req.target.host, req.target.port, req.dst_schema),
    );

    // 事件转发 + 落库
    let app2 = app.clone();
    let tid = task_id.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = on_event.send(ev.clone());
            crate::store::record_event(app2.clone(), &tid, &ev);
        }
    });

    let result = engine::run_task(cfg, &report, sink, cancel).await;
    let _ = forwarder.await;

    {
        let mut tasks = state.tasks.lock().await;
        tasks.remove(&task_id);
    }

    match result {
        Ok(r) => {
            crate::store::finish_task(app.clone(), &task_id, &r);
            Ok(r)
        }
        Err(e) => {
            crate::store::fail_task(app.clone(), &task_id, &e.to_string());
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub async fn cancel_sync(
    state: State<'_, Arc<AppState>>,
    task_id: String,
) -> Result<bool, String> {
    let tasks = state.tasks.lock().await;
    match tasks.get(&task_id) {
        Some(token) => {
            token.cancel();
            Ok(true)
        }
        None => Ok(false),
    }
}

// ---------------------------------------------------------------- 历史

#[tauri::command]
pub fn list_tasks(app: tauri::AppHandle) -> Result<Vec<crate::store::TaskSummary>, String> {
    crate::store::list_tasks(app)
}

#[tauri::command]
pub fn task_logs(
    app: tauri::AppHandle,
    task_id: String,
    limit: Option<usize>,
) -> Result<Vec<crate::store::LogRow>, String> {
    crate::store::task_logs(app, &task_id, limit.unwrap_or(500))
}


// ---------------------------------------------------------------- 数据源

#[tauri::command]
pub fn list_connections() -> Result<Vec<crate::connections::SavedConnection>, String> {
    crate::connections::list()
}

/// 保存数据源。密码直接存 ~/.RowBridge/connections.json，
/// 不再走钥匙串（用户反馈每次弹授权框太烦）。
#[tauri::command]
pub fn save_connection(
    conn: crate::connections::SavedConnection,
) -> Result<Vec<crate::connections::SavedConnection>, String> {
    crate::connections::upsert(conn)
}

#[tauri::command]
pub fn remove_connection(
    connection_id: String,
) -> Result<Vec<crate::connections::SavedConnection>, String> {
    crate::connections::remove(&connection_id)
}

#[tauri::command]
pub fn remember_schema(connection_id: String, schema: String) -> Result<(), String> {
    crate::connections::remember_schema(&connection_id, &schema)
}

#[tauri::command]
pub fn connections_path() -> Result<String, String> {
    Ok(crate::connections::config_dir()?
        .join("connections.json")
        .display()
        .to_string())
}

/// 在 Finder 里打开配置目录，方便用户查看/备份。
#[tauri::command]
pub fn reveal_config_dir(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = crate::connections::config_dir()?;
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
