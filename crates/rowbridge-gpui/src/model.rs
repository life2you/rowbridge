//! UI 数据模型：连接面板、选项面板、表行、运行进度。

use std::collections::HashMap;

use gpui_kit::component::input::InputState;
use gpui_kit::*;
use rowbridge_core::config::{ConnConfig, CursorStart};
use rowbridge_core::connections::SavedConnection;
use tokio_util::sync::CancellationToken;

/// 哪一侧的数据库（源 / 目标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbSide {
    Src,
    Dst,
}

impl DbSide {
    pub fn title(self) -> &'static str {
        match self {
            DbSide::Src => "源数据库",
            DbSide::Dst => "目标数据库",
        }
    }
}

/// 全局游标模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    Full,
    Resume,
    Custom,
}

impl CursorMode {
    pub fn label(self) -> &'static str {
        match self {
            CursorMode::Full => "全量",
            CursorMode::Resume => "续跑",
            CursorMode::Custom => "自定义",
        }
    }
    #[allow(dead_code)]
    pub fn to_core(self) -> CursorStart {
        match self {
            CursorMode::Full => CursorStart::Full,
            CursorMode::Resume => CursorStart::Resume,
            CursorMode::Custom => CursorStart::Custom,
        }
    }
}

/// 三态开关：跟随全局 / 开 / 关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    Follow,
    On,
    Off,
}

impl Tri {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Tri::Follow => "跟随",
            Tri::On => "是",
            Tri::Off => "否",
        }
    }
    pub fn next(self) -> Tri {
        match self {
            Tri::Follow => Tri::On,
            Tri::On => Tri::Off,
            Tri::Off => Tri::Follow,
        }
    }
    pub fn to_option(self) -> Option<bool> {
        match self {
            Tri::Follow => None,
            Tri::On => Some(true),
            Tri::Off => Some(false),
        }
    }
    pub fn from_option(v: Option<bool>) -> Tri {
        match v {
            None => Tri::Follow,
            Some(true) => Tri::On,
            Some(false) => Tri::Off,
        }
    }
}

/// 一个数据库连接的表单面板。
pub struct ConnPanel {
    pub host: Entity<InputState>,
    pub port: Entity<InputState>,
    pub user: Entity<InputState>,
    pub password: Entity<InputState>,
    pub use_tls: bool,
    /// 关联的已保存数据源 id
    pub saved_id: Option<String>,
    /// 已保存数据源的显示名
    pub label: Option<SharedString>,
    /// (是否成功, 文案)
    pub note: Option<(bool, SharedString)>,
    pub busy: bool,
    /// 连接表单是否折叠（连接成功后可折叠以释放垂直空间给库表列表）
    #[allow(dead_code)]
    pub collapsed: bool,
}

impl ConnPanel {
    pub fn new(title_placeholder: &'static str, window: &mut Window, cx: &mut App) -> Self {
        let host = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("主机，例如 127.0.0.1")
                .default_value("127.0.0.1")
        });
        let port = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("端口")
                .default_value("3306")
        });
        let user = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(title_placeholder)
                .default_value("root")
        });
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("密码").masked(true));
        Self {
            host,
            port,
            user,
            password,
            use_tls: false,
            saved_id: None,
            label: None,
            note: None,
            busy: false,
            collapsed: false,
        }
    }

    pub fn inputs(&self) -> Vec<Entity<InputState>> {
        vec![
            self.host.clone(),
            self.port.clone(),
            self.user.clone(),
            self.password.clone(),
        ]
    }

    pub fn host_text(&self, cx: &App) -> String {
        self.host.read(cx).value().to_string()
    }
    pub fn port_text(&self, cx: &App) -> String {
        self.port.read(cx).value().to_string()
    }
    pub fn user_text(&self, cx: &App) -> String {
        self.user.read(cx).value().to_string()
    }
    pub fn password_text(&self, cx: &App) -> String {
        self.password.read(cx).value().to_string()
    }

    pub fn conn_config(&self, cx: &App) -> ConnConfig {
        ConnConfig {
            host: self.host_text(cx).trim().to_string(),
            port: self.port_text(cx).trim().parse().unwrap_or(3306),
            username: self.user_text(cx).trim().to_string(),
            password: self.password_text(cx),
            schema: None,
            use_tls: self.use_tls,
        }
    }

    pub fn apply_saved(&mut self, c: &SavedConnection, window: &mut Window, cx: &mut App) {
        self.saved_id = Some(c.id.clone());
        self.label = Some(c.name.clone().into());
        self.use_tls = c.use_tls;
        set_input(&self.host, c.host.clone(), window, cx);
        set_input(&self.port, c.port.to_string(), window, cx);
        set_input(&self.user, c.username.clone(), window, cx);
        set_input(&self.password, c.password.clone(), window, cx);
    }

    #[allow(dead_code)]
    pub fn display_name(&self, cx: &App) -> SharedString {
        match &self.label {
            Some(l) => l.clone(),
            None => format!(
                "{}@{}:{}",
                self.user_text(cx),
                self.host_text(cx),
                self.port_text(cx)
            )
            .into(),
        }
    }
}

/// 把文本写进输入框（不触发 Change 事件）。
pub fn set_input(
    state: &Entity<InputState>,
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    let value = value.into();
    state.update(cx, |state, cx| state.set_value(value, window, cx));
}

/// 批量参数与高级选项。
pub struct OptionsPanel {
    pub parallelism: Entity<InputState>,
    pub batch_size: Entity<InputState>,
    pub max_batch_size: Entity<InputState>,
    pub global_start: Entity<InputState>,
    pub global_end: Entity<InputState>,
    pub cursor: CursorMode,
    pub adaptive_batch: bool,
    pub create_table: bool,
    pub create_database: bool,
    pub strip_foreign_keys: bool,
    pub strip_auto_increment: bool,
    pub map_collation: bool,
    pub add_missing_columns: bool,
    pub disable_foreign_key_checks: bool,
    pub truncate_target: bool,
    pub relax_sql_mode: bool,
    pub continue_on_error: bool,
}

impl OptionsPanel {
    pub fn new(window: &mut Window, cx: &mut impl AppContext) -> Self {
        let parallelism = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("并发")
                .default_value("2")
        });
        let batch_size = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("批大小")
                .default_value("1000")
        });
        let max_batch_size = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("批大小上限")
                .default_value("10000")
        });
        let global_start = cx.new(|cx| InputState::new(window, cx).placeholder("起始 ID"));
        let global_end = cx.new(|cx| InputState::new(window, cx).placeholder("结束 ID"));
        Self {
            parallelism,
            batch_size,
            max_batch_size,
            global_start,
            global_end,
            cursor: CursorMode::Full,
            adaptive_batch: true,
            create_table: true,
            create_database: true,
            strip_foreign_keys: true,
            strip_auto_increment: true,
            map_collation: true,
            add_missing_columns: true,
            disable_foreign_key_checks: false,
            truncate_target: false,
            relax_sql_mode: false,
            continue_on_error: true,
        }
    }

    pub fn inputs(&self) -> Vec<Entity<InputState>> {
        vec![
            self.parallelism.clone(),
            self.batch_size.clone(),
            self.max_batch_size.clone(),
            self.global_start.clone(),
            self.global_end.clone(),
        ]
    }

    pub fn usize_of(&self, state: &Entity<InputState>, cx: &App, default: usize) -> usize {
        let raw = state.read(cx).value().to_string();
        raw.trim().parse().unwrap_or(default)
    }

    pub fn text_of(&self, state: &Entity<InputState>, cx: &App) -> Option<String> {
        let raw = state.read(cx).value().to_string();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

/// 单表的游标 / 目标表覆盖。
#[derive(Debug, Clone, Default)]
pub struct TableOverride {
    pub dst_table: Option<String>,
    pub cursor: Option<CursorStart>,
    pub start_id: Option<String>,
    pub end_id: Option<String>,
    pub filter: Option<String>,
    pub truncate: Option<bool>,
    pub create_table: Option<bool>,
    pub strip_foreign_keys: Option<bool>,
    pub strip_auto_increment: Option<bool>,
    pub map_collation: Option<bool>,
    pub continue_on_error: Option<bool>,
}

impl TableOverride {
    #[allow(dead_code)]
    pub fn has_any(&self) -> bool {
        matches!(self.cursor, Some(CursorStart::Resume | CursorStart::Custom))
            || self.start_id.is_some()
            || self.end_id.is_some()
            || self.filter.is_some()
            || self.truncate.is_some()
            || self.create_table.is_some()
            || self.strip_foreign_keys.is_some()
            || self.strip_auto_increment.is_some()
            || self.map_collation.is_some()
            || self.continue_on_error.is_some()
    }
}

/// 表清单里的一行。
#[derive(Debug, Clone)]
pub struct TableRow {
    pub name: String,
    pub rows_estimate: u64,
    pub primary_key: Option<String>,
    pub source_max_pk: Option<String>,
    pub unsupported: Option<String>,
    pub selected: bool,
    pub over: TableOverride,
}

/// 运行日志项。
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: SharedString,
    pub level: SharedString,
    pub message: SharedString,
}

/// 引擎事件的 UI 视图。
#[derive(Debug, Clone)]
pub struct TableProgress {
    pub status: &'static str,
    pub rows: u64,
    pub planned: u64,
    pub rps: f64,
    pub cursor: String,
    pub error: Option<String>,
}

impl TableProgress {
    pub fn pending(planned: u64) -> Self {
        Self {
            status: "pending",
            rows: 0,
            planned,
            rps: 0.0,
            cursor: String::new(),
            error: None,
        }
    }
    pub fn percent(&self) -> f32 {
        if self.planned == 0 {
            if self.status == "done" {
                100.0
            } else {
                0.0
            }
        } else {
            (self.rows as f32 * 100.0 / self.planned as f32).clamp(0.0, 100.0)
        }
    }
}

/// 主视图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sync,
    Active,
    Sources,
    Log,
}

/// 任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

impl TaskStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, TaskStatus::Running | TaskStatus::Paused)
    }

    /// SQLite 里存的字符串。
    pub fn store_key(&self) -> &'static str {
        match self {
            TaskStatus::Running => "running",
            TaskStatus::Paused => "paused",
            TaskStatus::Completed => "done",
            TaskStatus::Cancelled => "cancelled",
            TaskStatus::Failed => "failed",
        }
    }

    pub fn from_store_key(key: &str) -> TaskStatus {
        match key {
            "running" => TaskStatus::Running,
            "paused" => TaskStatus::Paused,
            "cancelled" => TaskStatus::Cancelled,
            "failed" => TaskStatus::Failed,
            _ => TaskStatus::Completed,
        }
    }

    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            TaskStatus::Running => "运行中",
            TaskStatus::Paused => "已暂停",
            TaskStatus::Completed => "已完成",
            TaskStatus::Cancelled => "已取消",
            TaskStatus::Failed => "失败",
        }
    }
}

/// 任务日志项。
#[derive(Debug, Clone)]
pub struct TaskLogEntry {
    pub time: SharedString,
    pub level: SharedString,
    #[allow(dead_code)]
    pub table: Option<String>,
    pub message: SharedString,
}

/// 同步任务对象。
#[derive(Clone)]
pub struct SyncTask {
    pub id: String,
    pub name: String,
    pub created_at: String,
    /// 源数据源展示名（如「生产只读」）
    pub src_name: String,
    /// 目标数据源展示名
    pub dst_name: String,
    /// 源实例 host:port
    pub src_ep: String,
    /// 目标实例 host:port
    pub dst_ep: String,
    pub src_schema: String,
    pub dst_schema: String,
    pub tables: Vec<String>,
    pub status: TaskStatus,
    pub cancel: CancellationToken,
    pub pause: rowbridge_core::engine::PauseController,
    pub progress: HashMap<String, TableProgress>,
    pub logs: Vec<TaskLogEntry>,
    pub error: Option<String>,
}

impl SyncTask {
    /// 从 SQLite 历史恢复的任务卡片（进度/日志默认空，选中时再按需拉日志）。
    pub fn from_history(
        id: String,
        src: String,
        dst: String,
        status: &str,
        _total_rows: u64,
        started_at: i64,
        error: Option<String>,
    ) -> SyncTask {
        let created_at = chrono::DateTime::from_timestamp(started_at, 0)
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_default();
        // 历史格式是「实例/库」两个字段，展示名与地址只能取到实例部分。
        let src_name = src.clone();
        let dst_name = dst.clone();
        let (src_ep, src_schema) = split_endpoint(&src);
        let (dst_ep, dst_schema) = split_endpoint(&dst);
        // 历史日志直接带上：任务卡片在左侧点开就能看到上次的日志
        let logs = rowbridge_core::store::task_logs(&id, 500)
            .unwrap_or_default()
            .into_iter()
            .map(|r| TaskLogEntry {
                time: chrono::DateTime::from_timestamp(r.ts / 1000, 0)
                    .map(|t| t.format("%H:%M:%S").to_string())
                    .unwrap_or_default()
                    .into(),
                level: r.level.into(),
                table: r.table_name,
                message: r.message.into(),
            })
            .collect();
        // 历史库里没有逐表进度，只有总行数：合成一条伪表进度，
        // 让卡片展示「x/y 行 · 1/1 表」而不是误导性的 0/0。
        let mut progress = HashMap::new();
        let mut tables = Vec::new();
        if _total_rows > 0 {
            let mut p = TableProgress::pending(_total_rows);
            p.rows = _total_rows;
            p.status = "done";
            progress.insert("（全部表）".to_string(), p);
            tables.push("（全部表）".to_string());
        }
        SyncTask {
            name: format!("{src_schema} → {dst_schema}"),
            created_at,
            src_name,
            dst_name,
            src_ep: src_ep.to_string(),
            dst_ep: dst_ep.to_string(),
            src_schema: src_schema.to_string(),
            dst_schema: dst_schema.to_string(),
            tables,
            status: TaskStatus::from_store_key(status),
            cancel: CancellationToken::new(),
            pause: rowbridge_core::engine::PauseController::new(),
            progress,
            logs,
            error,
            id,
        }
    }
}

/// 拆「host:port/schema」。
fn split_endpoint(ep: &str) -> (&str, &str) {
    match ep.split_once('/') {
        Some((h, s)) => (h, s),
        None => (ep, ""),
    }
}

impl SyncTask {
    pub fn total_done(&self) -> u64 {
        self.progress.values().map(|p| p.rows).sum()
    }

    pub fn total_planned(&self) -> u64 {
        self.progress.values().map(|p| p.planned).sum()
    }

    pub fn done_tables_count(&self) -> usize {
        self.progress
            .values()
            .filter(|p| p.status == "done")
            .count()
    }

    pub fn is_active(&self) -> bool {
        self.status.is_active()
    }

    /// 数据源摘要：`生产只读 → AI服务5.7`。
    pub fn routes(&self) -> String {
        format!("{} → {}", self.src_name, self.dst_name)
    }

    /// 表摘要：`accounts 等 3 张表` / `accounts, users`。
    pub fn tables_summary(&self) -> String {
        match self.tables.len() {
            0 => "无表".to_string(),
            1 => self.tables[0].clone(),
            2 => self.tables.join(", "),
            n => format!("{} 等 {} 张表", self.tables[0], n),
        }
    }
}

/// 人类可读的行数。
pub fn human_rows(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.2} 亿", n as f64 / 1e8)
    } else if n >= 10_000 {
        format!("{:.1} 万", n as f64 / 1e4)
    } else {
        format!("{n}")
    }
}
