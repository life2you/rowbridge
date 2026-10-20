//! 值的无损搬运与游标处理。
//!
//! 这个模块是整个工具正确性的地基：
//!
//! 1. **不做类型解码**。`mysql_async` 的二进制协议已经把每个值解析成具体的
//!    [`Value`] 枚举变体，我们读出来直接原样塞回目标端即可。绝不经过 `String`
//!    中转，也不做时区换算——`DATETIME` 就按字面搬。
//!
//! 2. **游标复用原始 Value**。主键游标不是"字符串化再解析"，而是保留从源行里
//!    读到的那个 `Value`。原因是 `bigint unsigned` 的上半段（> i64::MAX）
//!    用 i64 存会溢出，用字符串比较又会对不齐。直接复用 `Value` 才能无损。

use mysql_async::Value;

/// 同步游标。
///
/// `None` 表示"还没开始"——此时第一批查询**不带 WHERE 子句**。
/// 这是个容易忽略的边界：如果用一个 `MIN(pk) - 1` 之类的哨兵值，
/// 主键含 0 或负数时就会算错，而 `None` 对所有整型都天然正确。
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor(pub Option<Value>);

impl Cursor {
    /// 起始游标：第一批不带 WHERE。
    pub fn start() -> Self {
        Cursor(None)
    }

    /// 从目标表 MAX(pk) 续传；NULL（空表）等价于从头开始。
    pub fn from_max_pk(v: Option<Value>) -> Self {
        match v {
            Some(Value::NULL) | None => Cursor(None),
            Some(v) => Cursor(Some(v)),
        }
    }

    pub fn advance(&mut self, v: Value) {
        debug_assert!(!matches!(v, Value::NULL), "主键不应为 NULL");
        self.0 = Some(v);
    }

    pub fn is_start(&self) -> bool {
        self.0.is_none()
    }

    /// 当前游标的数值表示，仅用于和 `end_id` 比较。非整型返回 None。
    pub fn as_i128(&self) -> Option<i128> {
        self.0.as_ref().and_then(value_as_i128)
    }

    /// 用于日志/进度展示的字符串。
    pub fn display(&self) -> String {
        match &self.0 {
            None => "(起始)".to_string(),
            Some(v) => value_to_display(v),
        }
    }
}

/// 把整型 `Value` 归一化成 `i128`。
///
/// 选 `i128` 是因为它能同时容纳 `i64` 全域和 `u64` 全域
/// （`u64::MAX ≈ 1.8e19 < i128::MAX ≈ 1.7e38`），做大小比较不会溢出。
pub fn value_as_i128(v: &Value) -> Option<i128> {
    match v {
        Value::Int(i) => Some(*i as i128),
        Value::UInt(u) => Some(*u as i128),
        Value::Bytes(b) => std::str::from_utf8(b).ok()?.trim().parse::<i128>().ok(),
        _ => None,
    }
}

/// 把用户输入的字符串解析成与主键列类型匹配的 `Value`。
///
/// `unsigned` 决定用 `UInt` 还是 `Int`——这两者在 MySQL 参数绑定里是不同的编码，
/// 对 `bigint unsigned` 列传 `Int` 会在超过 i64::MAX 时出错。
pub fn parse_cursor_value(input: &str, unsigned: bool) -> crate::Result<Value> {
    let s = input.trim();
    if s.is_empty() {
        return Err(crate::Error::Config("游标值不能为空".into()));
    }
    if unsigned {
        s.parse::<u64>()
            .map(Value::UInt)
            .map_err(|_| crate::Error::Config(format!("`{s}` 不是合法的无符号整数")))
    } else {
        s.parse::<i64>()
            .map(Value::Int)
            .map_err(|_| crate::Error::Config(format!("`{s}` 不是合法的整数")))
    }
}

/// 值的可读展示（日志用），长文本截断。
pub fn value_to_display(v: &Value) -> String {
    match v {
        Value::NULL => "NULL".into(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(f) => f.to_string(),
        Value::Date(y, mo, d, h, mi, s, us) => {
            if *us == 0 {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            } else {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}")
            }
        }
        Value::Time(neg, d, h, mi, s, us) => {
            let sign = if *neg { "-" } else { "" };
            let hours = *d as u64 * 24 + *h as u64;
            if *us == 0 {
                format!("{sign}{hours:02}:{mi:02}:{s:02}")
            } else {
                format!("{sign}{hours:02}:{mi:02}:{s:02}.{us:06}")
            }
        }
        Value::Bytes(b) => {
            let s = String::from_utf8_lossy(b);
            if s.chars().count() > 64 {
                let truncated: String = s.chars().take(64).collect();
                format!("{truncated}… ({})", b.len())
            } else {
                s.to_string()
            }
        }
    }
}

/// 估算一个值在 MySQL 文本协议下的字节数，用于 `max_allowed_packet` 预算。
pub fn value_wire_size(v: &Value) -> usize {
    match v {
        Value::NULL => 4,
        Value::Int(_) | Value::UInt(_) => 20,
        Value::Float(_) => 14,
        Value::Double(_) => 24,
        Value::Date(..) => 27,
        Value::Time(..) => 20,
        // 需要转义，最坏情况每个字节膨胀成 `\x` 或 `\0`，按 2 倍估。
        Value::Bytes(b) => b.len() * 2 + 2,
    }
}

/// 反引号转义标识符。`` a`b `` → `a``b`。
pub fn quote_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('`');
    for ch in name.chars() {
        if ch == '`' {
            out.push('`');
        }
        out.push(ch);
    }
    out.push('`');
    out
}

/// `schema`.`table` 形式的限定名。
pub fn qualified(schema: &str, table: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_beyond_i64_stays_lossless() {
        let v = Value::UInt(u64::MAX);
        assert_eq!(value_as_i128(&v), Some(u64::MAX as i128));
        assert_eq!(value_to_display(&v), "18446744073709551615");
    }

    #[test]
    fn cursor_start_has_no_where() {
        let c = Cursor::start();
        assert!(c.is_start());
        assert_eq!(c.display(), "(起始)");
    }

    #[test]
    fn null_max_pk_means_start() {
        assert!(Cursor::from_max_pk(Some(Value::NULL)).is_start());
        assert!(Cursor::from_max_pk(None).is_start());
        assert!(!Cursor::from_max_pk(Some(Value::Int(5))).is_start());
    }

    #[test]
    fn parses_custom_cursor_by_sign() {
        assert_eq!(
            parse_cursor_value("42", false).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            parse_cursor_value("18446744073709551615", true).unwrap(),
            Value::UInt(u64::MAX)
        );
        assert!(parse_cursor_value("-1", true).is_err());
        assert!(parse_cursor_value("abc", false).is_err());
        assert!(parse_cursor_value("  ", false).is_err());
    }

    #[test]
    fn quote_ident_escapes_backtick() {
        assert_eq!(quote_ident("a`b"), "`a``b`");
        assert_eq!(qualified("db", "t"), "`db`.`t`");
    }

    #[test]
    fn display_truncates_long_bytes() {
        let v = Value::Bytes("x".repeat(200).into_bytes());
        let s = value_to_display(&v);
        assert!(s.ends_with("(200)"));
        assert!(s.chars().count() < 90);
    }
}
