//! 连接与任务配置。

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// 一个 MySQL 连接的目标端点。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConnConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 可选：限制在该 schema 下（用于 url 的 database 段）。
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub use_tls: bool,
}

fn default_port() -> u16 {
    3306
}

impl ConnConfig {
    /// 构造 mysql_async 连接参数。
    ///
    /// 注意：用户名和密码都要做百分号编码，否则含 `@`、`:`、`/` 的密码会解析失败。
    pub fn to_url(&self) -> Result<mysql_async::Opts> {
        if self.host.trim().is_empty() {
            return Err(Error::Config("主机不能为空".into()));
        }
        if self.username.trim().is_empty() {
            return Err(Error::Config("用户名不能为空".into()));
        }

        let user = percent_encode(&self.username);
        let pass = percent_encode(&self.password);
        let scheme = if self.use_tls { "mysqls" } else { "mysql" };

        let mut url = format!(
            "{scheme}://{user}:{pass}@{}:{}",
            self.host.trim(),
            self.port
        );
        if let Some(schema) = self.schema.as_ref().filter(|s| !s.is_empty()) {
            url.push('/');
            url.push_str(&percent_encode(schema));
        }

        Ok(mysql_async::Opts::from_url(&url)?)
    }

    /// 用于日志/UI 显示的脱敏标识。
    pub fn display(&self) -> String {
        format!("{}@{}{}", self.username, self.host, if self.port != 3306 { format!(":{}", self.port) } else { String::new() })
    }
}

/// RFC 3986 未保留字符之外的字节全部编码。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 主键游标起点策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CursorStart {
    /// 从源表最小值开始（全量）。
    #[default]
    Full,
    /// 从目标表 MAX(pk) 继续（断点续跑）。
    Resume,
    /// 用户指定。
    Custom,
}

/// 写入冲突策略。首版固定 UPSERT，保留枚举以便后续扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// INSERT ... ON DUPLICATE KEY UPDATE（幂等，可重跑）
    #[default]
    Upsert,
    /// INSERT IGNORE（只补缺失行）
    InsertIgnore,
    /// 纯 INSERT（遇重复即报错）
    Insert,
}

/// 单表同步配置。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TableTaskConfig {
    pub table: String,
    /// 源库名
    pub src_schema: String,
    /// 目标库名
    pub dst_schema: String,
    /// 目标表名（默认与源同名）
    #[serde(default)]
    pub dst_table: Option<String>,
    /// 要同步的列；为空表示源表全部列
    #[serde(default)]
    pub columns: Vec<String>,

    /// 本表独立的游标模式；None 表示跟随全局 TaskOptions。
    #[serde(default)]
    pub cursor_start: Option<CursorStart>,
    /// 本表独立的起始 id（cursor_start 解析为 Custom 时生效）
    #[serde(default)]
    pub start_id: Option<String>,
    /// 本表独立的结束 id（含）
    #[serde(default)]
    pub end_id: Option<String>,
}

impl TableTaskConfig {
    /// 解析本表实际生效的游标模式：表级优先，其次全局。
    pub fn effective_cursor_start(&self, global: &TaskOptions) -> CursorStart {
        self.cursor_start.unwrap_or(global.cursor_start)
    }

    /// 解析本表实际生效的起始 id。
    ///
    /// 表级显式设了就用表级（包括设成空串表示「不限」）；
    /// 否则回落到全局。返回 owned 值是为了避开两个来源的生命周期纠缠。
    pub fn effective_start_id(&self, global: &TaskOptions) -> Option<String> {
        self.start_id
            .clone()
            .or_else(|| global.start_id.clone())
    }

    /// 解析本表实际生效的结束 id。
    pub fn effective_end_id(&self, global: &TaskOptions) -> Option<String> {
        self.end_id.clone().or_else(|| global.end_id.clone())
    }

    /// 是否有表级游标覆盖。
    pub fn has_cursor_override(&self) -> bool {
        self.cursor_start.is_some() || self.start_id.is_some() || self.end_id.is_some()
    }
}

impl TableTaskConfig {
    pub fn dst_table_name(&self) -> &str {
        self.dst_table.as_deref().unwrap_or(&self.table)
    }
}

/// 整个任务的配置。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskConfig {
    pub source: ConnConfig,
    pub target: ConnConfig,
    pub tables: Vec<TableTaskConfig>,

    #[serde(default)]
    pub options: TaskOptions,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskOptions {
    /// 单表并发数上限（多表并行）。
    #[serde(default = "default_parallelism")]
    pub parallelism: usize,
    /// 初始批大小
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// 批大小上限
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
    /// 是否自适应调整批大小
    #[serde(default = "default_true")]
    pub adaptive_batch: bool,
    #[serde(default)]
    pub cursor_start: CursorStart,
    #[serde(default)]
    pub write_mode: WriteMode,
    /// 结束游标（含）；None 表示同步到最后。
    #[serde(default)]
    pub end_id: Option<String>,
    /// 自定义起始游标（cursor_start == Custom 时使用）。
    #[serde(default)]
    pub start_id: Option<String>,

    /// 目标表不存在时自动建表
    #[serde(default = "default_true")]
    pub create_table: bool,
    /// 目标库不存在时自动建库
    #[serde(default = "default_true")]
    pub create_database: bool,
    /// 建表时移除外键约束
    #[serde(default = "default_true")]
    pub strip_foreign_keys: bool,
    /// 建表时移除 AUTO_INCREMENT= 选项
    #[serde(default = "default_true")]
    pub strip_auto_increment: bool,
    /// 建表时做 collation 降级映射
    #[serde(default = "default_true")]
    pub map_collation: bool,

    /// 会话级 foreign_key_checks=0
    #[serde(default)]
    pub disable_foreign_key_checks: bool,
    /// 会话级 sql_mode=''
    #[serde(default)]
    pub relax_sql_mode: bool,
    /// 单表失败后是否继续其它表
    #[serde(default = "default_true")]
    pub continue_on_error: bool,
    /// 写入失败的重试次数
    #[serde(default = "default_retries")]
    pub max_retries: u32,
}

fn default_parallelism() -> usize {
    2
}
fn default_batch_size() -> usize {
    1000
}
fn default_max_batch_size() -> usize {
    10_000
}
fn default_retries() -> u32 {
    3
}
fn default_true() -> bool {
    true
}

impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            parallelism: default_parallelism(),
            batch_size: default_batch_size(),
            max_batch_size: default_max_batch_size(),
            adaptive_batch: true,
            cursor_start: CursorStart::Full,
            write_mode: WriteMode::Upsert,
            end_id: None,
            start_id: None,
            create_table: true,
            create_database: true,
            strip_foreign_keys: true,
            strip_auto_increment: true,
            map_collation: true,
            disable_foreign_key_checks: false,
            relax_sql_mode: false,
            continue_on_error: true,
            max_retries: default_retries(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_special_chars_in_password() {
        let c = ConnConfig {
            host: "127.0.0.1".into(),
            port: 3306,
            username: "u@x".into(),
            password: "p@ss:w/rd#1".into(),
            schema: None,
            use_tls: false,
        };
        let url = c.to_url().unwrap();
        assert_eq!(url.user(), Some("u@x"));
        assert_eq!(url.pass(), Some("p@ss:w/rd#1"));
        assert_eq!(url.ip_or_hostname(), "127.0.0.1");
    }

    #[test]
    fn plain_config_roundtrips() {
        let c = ConnConfig {
            host: "localhost".into(),
            port: 3306,
            username: "root".into(),
            password: "123456".into(),
            schema: Some("db_a".into()),
            use_tls: false,
        };
        let url = c.to_url().unwrap();
        assert_eq!(url.db_name(), Some("db_a"));
    }

    fn task(table: &str, cs: Option<CursorStart>, start: Option<&str>, end: Option<&str>) -> TableTaskConfig {
        TableTaskConfig {
            table: table.into(),
            src_schema: "db_a".into(),
            dst_schema: "db_b".into(),
            dst_table: None,
            columns: Vec::new(),
            cursor_start: cs,
            start_id: start.map(|s| s.to_string()),
            end_id: end.map(|s| s.to_string()),
        }
    }

    #[test]
    fn table_cursor_overrides_global() {
        let global = TaskOptions {
            cursor_start: CursorStart::Full,
            start_id: Some("100".into()),
            end_id: Some("200".into()),
            ..Default::default()
        };
        let t = task("a", Some(CursorStart::Custom), Some("500"), Some("900"));
        assert_eq!(t.effective_cursor_start(&global), CursorStart::Custom);
        assert_eq!(t.effective_start_id(&global).as_deref(), Some("500"));
        assert_eq!(t.effective_end_id(&global).as_deref(), Some("900"));
        assert!(t.has_cursor_override());
    }

    #[test]
    fn table_without_override_follows_global() {
        let global = TaskOptions {
            cursor_start: CursorStart::Resume,
            start_id: None,
            end_id: Some("200".into()),
            ..Default::default()
        };
        let t = task("a", None, None, None);
        assert_eq!(t.effective_cursor_start(&global), CursorStart::Resume);
        assert_eq!(t.effective_end_id(&global).as_deref(), Some("200"));
        assert!(!t.has_cursor_override());
    }

    #[test]
    fn empty_table_end_id_means_unlimited_not_fallback() {
        // 表级显式设成空串 = 「不限」，不能被全局的 end_id 顶掉，
        // 否则用户清空某一行的结束 id 会莫名其妙地又用上全局值。
        let global = TaskOptions {
            end_id: Some("200".into()),
            ..Default::default()
        };
        let t = task("a", None, None, Some(""));
        assert_eq!(t.effective_end_id(&global).as_deref(), Some(""));
    }

    #[test]
    fn rejects_empty_host() {
        let c = ConnConfig {
            host: "  ".into(),
            port: 3306,
            username: "root".into(),
            password: String::new(),
            schema: None,
            use_tls: false,
        };
        assert!(c.to_url().is_err());
    }
}
