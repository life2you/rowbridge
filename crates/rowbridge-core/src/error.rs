use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("MySQL 错误 [{code}] {state}: {message}")]
    MySql {
        code: u16,
        state: String,
        message: String,
    },

    #[error("连接失败: {0}")]
    Connect(String),

    #[error("配置无效: {0}")]
    Config(String),

    #[error("预检未通过: {0}")]
    Preflight(String),

    #[error("目标表 {table} 缺少非空且无默认值的列: {columns:?}")]
    MissingRequiredColumns { table: String, columns: Vec<String> },

    #[error("表 {table} 没有可用的单列整型主键（实际: {actual}）")]
    UnsupportedPrimaryKey { table: String, actual: String },

    #[error("DDL 改写失败: {0}")]
    DdlRewrite(String),

    #[error("任务已取消")]
    Cancelled,

    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// 是否值得重试（连接类、资源类错误）。
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::MySql { code, .. } => matches!(
                code,
                2006 | 2013 // 连接断开
                | 1153      // 包过大
                | 1205      // 锁等待超时
                | 1213      // 死锁
                | 1040      // too many connections
            ),
            Error::Io(_) => true,
            _ => false,
        }
    }

    /// 是否应该缩减批大小后重试。
    pub fn should_shrink_batch(&self) -> bool {
        matches!(self, Error::MySql { code: 1153 | 2006, .. })
    }

    pub fn code(&self) -> Option<u16> {
        match self {
            Error::MySql { code, .. } => Some(*code),
            _ => None,
        }
    }
}

impl From<mysql_async::Error> for Error {
    fn from(e: mysql_async::Error) -> Self {
        match e {
            mysql_async::Error::Server(se) => Error::MySql {
                code: se.code,
                state: se.state.clone(),
                message: se.message.clone(),
            },
            mysql_async::Error::Io(io) => match io {
                mysql_async::IoError::Io(e) => Error::Io(e),
                other => Error::Other(other.to_string()),
            },
            other => Error::Other(other.to_string()),
        }
    }
}

impl From<mysql_async::UrlError> for Error {
    fn from(e: mysql_async::UrlError) -> Self {
        Error::Config(format!("连接串无效: {e}"))
    }
}
