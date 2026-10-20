//! `SHOW CREATE TABLE` 改写流水线。
//!
//! 目标表不存在时，我们复制源表结构。但源表 DDL **不能原样执行**：
//!
//! | 问题 | 原样执行的后果 |
//! |------|----------------|
//! | 带着 `AUTO_INCREMENT=12345` | 目标表从源表当前值起跳，主键出现大段空洞 |
//! | 带着外键约束 | 被引用表可能还没建，直接报 1215/1824 |
//! | 8.0 的 `utf8mb4_0900_ai_ci` | 目标端 5.7 / MariaDB 直接建表失败 |
//! | 8.0 独有的 `CHECK`、函数索引、`INVISIBLE` | 低版本语法错误 |
//!
//! 每一步都可单独开关，改写结果以 diff 形式展示给用户确认。

use serde::{Deserialize, Serialize};

use crate::value::quote_ident;

/// 改写选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteOptions {
    /// 目标 schema
    pub dst_schema: String,
    /// 目标表名
    pub dst_table: String,
    /// 移除 AUTO_INCREMENT=
    pub strip_auto_increment: bool,
    /// 移除外键约束
    pub strip_foreign_keys: bool,
    /// collation 降级映射
    pub map_collation: bool,
    /// 目标服务端主版本号，用于判断是否需要降级
    pub target_version: (u16, u16, u16),
    /// 目标是否为 MariaDB
    pub target_is_mariadb: bool,
}

/// 改写结果，包括最终 DDL 和每一步做过的改动（给 UI 展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteResult {
    pub ddl: String,
    pub changes: Vec<RewriteChange>,
    /// 无法自动解决、需要用户决策的问题
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteChange {
    pub kind: String,
    pub detail: String,
}

/// 执行改写流水线。
pub fn rewrite_create_table(original: &str, opts: &RewriteOptions) -> RewriteResult {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();

    let mut ddl = original.trim().to_string();

    // 1. 表名替换：定位第一行的 CREATE TABLE `x` ( 并换成目标限定名。
    ddl = replace_table_name(&ddl, &opts.dst_schema, &opts.dst_table);
    changes.push(RewriteChange {
        kind: "table_name".into(),
        detail: format!("表名 → {}.{}", opts.dst_schema, opts.dst_table),
    });

    // 2. AUTO_INCREMENT=
    if opts.strip_auto_increment {
        if let Some(new_ddl) = strip_auto_increment_option(&ddl) {
            changes.push(RewriteChange {
                kind: "auto_increment".into(),
                detail: "移除表级 AUTO_INCREMENT 起始值（主键值由数据显式写入）".into(),
            });
            ddl = new_ddl;
        }
    }

    // 3. 外键约束
    if opts.strip_foreign_keys {
        let (new_ddl, removed) = strip_foreign_keys(&ddl);
        if !removed.is_empty() {
            changes.push(RewriteChange {
                kind: "foreign_keys".into(),
                detail: format!("移除 {} 个外键约束: {}", removed.len(), removed.join(", ")),
            });
            ddl = new_ddl;
        }
    }

    // 4. collation 降级
    if opts.map_collation {
        let (new_ddl, mapped) = map_collations(&ddl, opts.target_version, opts.target_is_mariadb);
        if !mapped.is_empty() {
            changes.push(RewriteChange {
                kind: "collation".into(),
                detail: format!("collation 降级: {}", mapped.join(", ")),
            });
            ddl = new_ddl;
        }
    }

    // 5. 高版本特性检测（只警告，不改）
    let target_major_minor = (opts.target_version.0, opts.target_version.1);
    if !opts.target_is_mariadb && target_major_minor < (8, 0) {
        let lower = ddl.to_ascii_lowercase();
        if lower.contains(" check (") {
            warnings.push("源表含 CHECK 约束，目标端 MySQL < 8.0.16 会忽略该约束".into());
        }
        if lower.contains("invisible") {
            warnings.push("源表含 INVISIBLE 列，目标端 MySQL < 8.0.23 不支持".into());
        }
        if lower.contains("default (") {
            warnings.push("源表含表达式默认值 `DEFAULT (expr)`，目标端 MySQL < 8.0.13 不支持".into());
        }
        if lower.contains("engine=innodb") && lower.contains("row_format") {
            // 无害
        }
    }
    if opts.target_is_mariadb && ddl.to_ascii_lowercase().contains("json") {
        warnings.push("目标为 MariaDB：JSON 列会被建成 LONGTEXT 别名".into());
    }

    RewriteResult {
        ddl,
        changes,
        warnings,
    }
}

/// 把 `CREATE TABLE `old` (` 换成目标限定名。
fn replace_table_name(ddl: &str, schema: &str, table: &str) -> String {
    let target = format!("{}.{}", quote_ident(schema), quote_ident(table));
    let lower = ddl.to_ascii_lowercase();

    let Some(prefix_idx) = lower.find("create table") else {
        return ddl.to_string();
    };
    let after_prefix = prefix_idx + "create table".len();
    let rest = &ddl[after_prefix..];

    // 跳过 IF NOT EXISTS / 空白
    let trimmed_start = rest.len() - rest.trim_start().len();
    let mut cursor = after_prefix + trimmed_start;
    let lower_rest = ddl[cursor..].to_ascii_lowercase();
    if lower_rest.starts_with("if not exists") {
        cursor += "if not exists".len();
    }
    let tail = &ddl[cursor..];
    let lead_ws = tail.len() - tail.trim_start().len();
    cursor += lead_ws;

    // 解析 `name` 或 name（可能带 schema 前缀）
    let tail = &ddl[cursor..];
    let name_end = match parse_qualified_name_end(tail) {
        Some(end) => end,
        None => return ddl.to_string(),
    };

    format!("{}{}{}", &ddl[..cursor], target, &tail[name_end..])
}

/// 解析一个可能带反引号、可能带 schema 前缀的表名，返回其结束位置（字节）。
fn parse_qualified_name_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut last_end = 0;

    loop {
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b'`' {
            i += 1;
            loop {
                if i >= bytes.len() {
                    return None;
                }
                if bytes[i] == b'`' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$') {
                i += 1;
            }
            if i == last_end {
                return None;
            }
        }
        last_end = i;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            continue;
        }
        return Some(i);
    }
}

/// 删掉 `AUTO_INCREMENT=12345` 这类表选项，同时清掉多余逗号/空格。
fn strip_auto_increment_option(ddl: &str) -> Option<String> {
    let lower = ddl.to_ascii_lowercase();
    let pos = lower.rfind("auto_increment=")?;
    let bytes = ddl.as_bytes();
    let mut end = pos + "auto_increment=".len();
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    // 同时吃掉后面可能的空格
    let mut full_end = end;
    while full_end < bytes.len() && (bytes[full_end] == b' ' || bytes[full_end] == b'\n') {
        full_end += 1;
    }
    let mut out = String::with_capacity(ddl.len());
    out.push_str(&ddl[..pos]);
    out.push_str(&ddl[full_end..]);
    // 去掉可能残留的行尾空格
    Some(out.trim_end().to_string())
}

/// 移除所有 `CONSTRAINT `x` FOREIGN KEY (...) REFERENCES ...`，返回被移除的约束名。
fn strip_foreign_keys(ddl: &str) -> (String, Vec<String>) {
    let mut removed = Vec::new();
    let mut out_lines: Vec<String> = Vec::new();

    let had_trailing_newline = ddl.ends_with('\n');
    let lines: Vec<&str> = ddl.lines().collect();
    let last_idx = lines.len().saturating_sub(1);

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        let is_fk = lower.starts_with("constraint")
            && lower.contains("foreign key")
            || (lower.starts_with("foreign key") && lower.contains("references"));
        if is_fk {
            if let Some(name) = extract_constraint_name(trimmed) {
                removed.push(name);
            } else {
                removed.push("(未命名外键)".into());
            }
            // 这一行被删了；如果它是最后一条（原本以逗号结尾），要给上一条去掉逗号。
            continue;
        }
        out_lines.push((*line).to_string());

        // 如果删掉的正好是最后一行且原行以逗号结尾，需要修正上一条。
        if idx == last_idx {
            // 最后一行不是外键，正常
        }
    }

    // 若最后一条有效数据行以逗号结尾但后面已无内容，去掉该逗号。
    if let Some(close_idx) = out_lines.iter().rposition(|l| l.trim_start().starts_with(')')) {
        if close_idx > 0 {
            let prev = out_lines[close_idx - 1].trim_end();
            if prev.ends_with(',') {
                let without = prev.trim_end_matches(',').to_string();
                out_lines[close_idx - 1] = without;
            }
        }
    }

    let mut joined = out_lines.join("\n");
    if had_trailing_newline {
        joined.push('\n');
    }
    (joined, removed)
}

fn extract_constraint_name(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let start = lower.find("constraint")? + "constraint".len();
    let rest = line[start..].trim_start();
    if let Some(stripped) = rest.strip_prefix('`') {
        let end = stripped.find('`')?;
        return Some(stripped[..end].to_string());
    }
    let end = rest.find(|c: char| c.is_whitespace())?;
    Some(rest[..end].to_string())
}

/// MySQL 8.0 的 utf8mb4 排序规则在低版本不存在，需要映射。
fn map_collations(
    ddl: &str,
    target_version: (u16, u16, u16),
    is_mariadb: bool,
) -> (String, Vec<String>) {
    let supports_0900 = !is_mariadb && target_version >= (8, 0, 0);
    if supports_0900 {
        return (ddl.to_string(), Vec::new());
    }

    // 8.0 独有 → 通用等价
    const MAP: &[(&str, &str)] = &[
        ("utf8mb4_0900_ai_ci", "utf8mb4_unicode_ci"),
        ("utf8mb4_0900_as_ci", "utf8mb4_unicode_ci"),
        ("utf8mb4_0900_as_cs", "utf8mb4_bin"),
        ("utf8mb4_0900_bin", "utf8mb4_bin"),
        ("utf8mb4_ja_0900_as_cs", "utf8mb4_unicode_ci"),
        ("utf8mb4_zh_0900_as_cs", "utf8mb4_unicode_ci"),
    ];

    let mut out = ddl.to_string();
    let mut applied = Vec::new();
    for (from, to) in MAP {
        if out.to_ascii_lowercase().contains(from) {
            let before = out.clone();
            out = replace_case_insensitive(&out, from, to);
            if out != before {
                applied.push(format!("{from} → {to}"));
            }
        }
    }
    (out, applied)
}

fn replace_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    let lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    let mut idx = 0;
    while let Some(found) = lower[idx..].find(&needle_lower) {
        let abs = idx + found;
        out.push_str(&haystack[last..abs]);
        out.push_str(replacement);
        last = abs + needle.len();
        idx = last;
    }
    out.push_str(&haystack[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RewriteOptions {
        RewriteOptions {
            dst_schema: "db_b".into(),
            dst_table: "a".into(),
            strip_auto_increment: true,
            strip_foreign_keys: true,
            map_collation: true,
            target_version: (8, 0, 36),
            target_is_mariadb: false,
        }
    }

    const SAMPLE: &str = "CREATE TABLE `a` (\n  `id` bigint unsigned NOT NULL AUTO_INCREMENT,\n  `name` varchar(64) COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB AUTO_INCREMENT=12345 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci";

    #[test]
    fn replaces_table_name() {
        let r = rewrite_create_table(SAMPLE, &opts());
        assert!(r.ddl.starts_with("CREATE TABLE `db_b`.`a` ("), "{}", r.ddl);
    }

    #[test]
    fn strips_auto_increment() {
        let r = rewrite_create_table(SAMPLE, &opts());
        assert!(!r.ddl.contains("AUTO_INCREMENT=12345"), "{}", r.ddl);
        // 列定义里的 AUTO_INCREMENT 必须保留
        assert!(r.ddl.contains("`id` bigint unsigned NOT NULL AUTO_INCREMENT"));
    }

    #[test]
    fn maps_collation_for_old_target() {
        let mut o = opts();
        o.target_version = (5, 7, 44);
        let r = rewrite_create_table(SAMPLE, &o);
        assert!(!r.ddl.contains("0900"), "{}", r.ddl);
        assert!(r.ddl.contains("utf8mb4_unicode_ci"));
    }

    #[test]
    fn keeps_collation_for_8_0_target() {
        let r = rewrite_create_table(SAMPLE, &opts());
        assert!(r.ddl.contains("utf8mb4_0900_ai_ci"), "{}", r.ddl);
    }

    #[test]
    fn strips_foreign_key_and_fixes_comma() {
        let ddl = "CREATE TABLE `a` (\n  `id` int NOT NULL,\n  `pid` int DEFAULT NULL,\n  PRIMARY KEY (`id`),\n  CONSTRAINT `fk_p` FOREIGN KEY (`pid`) REFERENCES `p` (`id`)\n) ENGINE=InnoDB";
        let r = rewrite_create_table(ddl, &opts());
        assert!(!r.ddl.contains("FOREIGN KEY"), "{}", r.ddl);
        // 去掉最后一条后，前一条不能留多余逗号
        assert!(r.ddl.contains("PRIMARY KEY (`id`)\n)"), "{}", r.ddl);
        assert!(r
            .changes
            .iter()
            .any(|c| c.kind == "foreign_keys" && c.detail.contains("fk_p")));
    }

    #[test]
    fn handles_backticked_and_unquoted_names() {
        let r = rewrite_create_table("CREATE TABLE `weird``name` (`id` int)", &opts());
        assert!(r.ddl.contains("`db_b`.`a`"), "{}", r.ddl);

        let r = rewrite_create_table("CREATE TABLE simple (id int)", &opts());
        assert!(r.ddl.contains("`db_b`.`a`"), "{}", r.ddl);
    }

    #[test]
    fn handles_if_not_exists() {
        let r = rewrite_create_table("CREATE TABLE IF NOT EXISTS `a` (`id` int)", &opts());
        assert!(r.ddl.contains("IF NOT EXISTS `db_b`.`a`"), "{}", r.ddl);
    }

    #[test]
    fn warns_on_low_version_features() {
        let mut o = opts();
        o.target_version = (5, 7, 44);
        let ddl = "CREATE TABLE `a` (\n `id` int,\n `v` int CHECK (v > 0)\n)";
        let r = rewrite_create_table(ddl, &o);
        assert!(r.warnings.iter().any(|w| w.contains("CHECK")));
    }
}
