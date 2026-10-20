//! `information_schema` 内省。

use serde::{Deserialize, Serialize};
use mysql_async::prelude::*;

use crate::value::quote_ident;
use crate::{Error, Result};

/// 整型主键类型白名单。
pub const INT_TYPES: &[&str] = &[
    "tinyint", "smallint", "mediumint", "int", "integer", "bigint",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchemaInfo {
    pub name: String,
    pub charset: Option<String>,
    pub collation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TableInfo {
    pub name: String,
    pub engine: Option<String>,
    pub rows_estimate: Option<u64>,
    pub table_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub column_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
    pub extra: String,
    pub charset: Option<String>,
    pub collation: Option<String>,
    /// 是否属于主键
    pub is_primary: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub column: String,
    pub seq: u32,
    pub non_unique: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TableDetail {
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    /// 主键列（按顺序）
    pub primary_key: Vec<String>,
    pub triggers: Vec<String>,
}

impl TableDetail {
    /// 首版只支持单列整型主键。
    pub fn pk_column(&self) -> std::result::Result<&ColumnInfo, Error> {
        if self.primary_key.is_empty() {
            return Err(Error::UnsupportedPrimaryKey {
                table: format!("{}.{}", self.schema, self.table),
                actual: "无主键".into(),
            });
        }
        if self.primary_key.len() > 1 {
            return Err(Error::UnsupportedPrimaryKey {
                table: format!("{}.{}", self.schema, self.table),
                actual: format!("复合主键 ({})", self.primary_key.join(", ")),
            });
        }
        let name = &self.primary_key[0];
        let col = self
            .columns
            .iter()
            .find(|c| &c.name == name)
            .ok_or_else(|| Error::Other(format!("主键列 {name} 不在列列表中")))?;
        if !INT_TYPES.contains(&col.data_type.to_ascii_lowercase().as_str()) {
            return Err(Error::UnsupportedPrimaryKey {
                table: format!("{}.{}", self.schema, self.table),
                actual: format!("主键 {} 类型为 {}", col.name, col.column_type),
            });
        }
        Ok(col)
    }

    pub fn is_unsigned_pk(&self) -> bool {
        self.pk_column()
            .map(|c| c.column_type.to_ascii_lowercase().contains("unsigned"))
            .unwrap_or(false)
    }

    /// 需要搬运的列（排除生成列——它们不能显式写入）。
    pub fn copyable_columns(&self) -> Vec<&ColumnInfo> {
        self.columns
            .iter()
            .filter(|c| {
                let extra = c.extra.to_ascii_lowercase();
                !extra.contains("virtual generated") && !extra.contains("stored generated")
            })
            .collect()
    }
}

/// 列出所有 schema（排除系统库）。
pub async fn list_schemas(conn: &mut mysql_async::Conn) -> Result<Vec<SchemaInfo>> {
    let sql = r#"
        SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME
        FROM information_schema.SCHEMATA
        WHERE SCHEMA_NAME NOT IN
            ('information_schema','performance_schema','mysql','sys')
        ORDER BY SCHEMA_NAME
    "#;
    let rows: Vec<(String, Option<String>, Option<String>)> = conn.query(sql).await?;
    Ok(rows
        .into_iter()
        .map(|(name, charset, collation)| SchemaInfo {
            name,
            charset,
            collation,
        })
        .collect())
}

pub async fn schema_exists(conn: &mut mysql_async::Conn, schema: &str) -> Result<bool> {
    let row: Option<(i64,)> = conn
        .exec_first(
            "SELECT 1 FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = ?",
            (schema,),
        )
        .await?;
    Ok(row.is_some())
}

/// 列出 schema 下的表，附带行数估算。
pub async fn list_tables(conn: &mut mysql_async::Conn, schema: &str) -> Result<Vec<TableInfo>> {
    let sql = r#"
        SELECT TABLE_NAME, ENGINE, TABLE_ROWS, TABLE_TYPE
        FROM information_schema.TABLES
        WHERE TABLE_SCHEMA = ?
        ORDER BY TABLE_NAME
    "#;
    let rows: Vec<(String, Option<String>, Option<u64>, Option<String>)> =
        conn.exec(sql, (schema,)).await?;
    Ok(rows
        .into_iter()
        .map(|(name, engine, rows_estimate, table_type)| TableInfo {
            name,
            engine,
            rows_estimate,
            table_type,
        })
        .collect())
}

pub async fn table_exists(conn: &mut mysql_async::Conn, schema: &str, table: &str) -> Result<bool> {
    let row: Option<(i64,)> = conn
        .exec_first(
            "SELECT 1 FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
            (schema, table),
        )
        .await?;
    Ok(row.is_some())
}

/// 读取表的完整结构。
pub async fn table_detail(
    conn: &mut mysql_async::Conn,
    schema: &str,
    table: &str,
) -> Result<TableDetail> {
    let col_rows: Vec<(
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
    )> = conn
        .exec(
            r#"
            SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT,
                   EXTRA, CHARACTER_SET_NAME, COLLATION_NAME
            FROM information_schema.COLUMNS
            WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
            ORDER BY ORDINAL_POSITION
            "#,
            (schema, table),
        )
        .await?;

    let idx_rows: Vec<(String, String, u32, u32)> = conn
        .exec(
            r#"
            SELECT INDEX_NAME, COLUMN_NAME, SEQ_IN_INDEX, NON_UNIQUE
            FROM information_schema.STATISTICS
            WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
            ORDER BY INDEX_NAME, SEQ_IN_INDEX
            "#,
            (schema, table),
        )
        .await?;

    let trig_rows: Vec<(String,)> = conn
        .exec(
            "SELECT TRIGGER_NAME FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA = ? AND EVENT_OBJECT_TABLE = ?",
            (schema, table),
        )
        .await?;

    if col_rows.is_empty() {
        return Err(Error::Other(format!("表 {schema}.{table} 不存在或没有列")));
    }

    // 复合主键的列顺序必须按 SEQ_IN_INDEX 排，否则拼出来的 WHERE 条件顺序不对。
    let mut pk_pairs: Vec<(u32, String)> = idx_rows
        .iter()
        .filter(|(name, _, _, _)| name == "PRIMARY")
        .map(|(_, col, seq, _)| (*seq, col.clone()))
        .collect();
    pk_pairs.sort_by_key(|(seq, _)| *seq);
    let primary_key: Vec<String> = pk_pairs.into_iter().map(|(_, c)| c).collect();

    let columns = col_rows
        .into_iter()
        .map(
            |(name, data_type, column_type, is_nullable, column_default, extra, charset, collation)| {
                ColumnInfo {
                    is_primary: primary_key.contains(&name),
                    name,
                    data_type,
                    column_type,
                    is_nullable: is_nullable.eq_ignore_ascii_case("YES"),
                    column_default,
                    extra,
                    charset,
                    collation,
                }
            },
        )
        .collect();

    let indexes = idx_rows
        .into_iter()
        .map(|(name, column, seq, non_unique)| IndexInfo {
            name,
            column,
            seq,
            non_unique: non_unique != 0,
        })
        .collect();

    Ok(TableDetail {
        schema: schema.to_string(),
        table: table.to_string(),
        columns,
        indexes,
        primary_key,
        triggers: trig_rows.into_iter().map(|(n,)| n).collect(),
    })
}

/// 读取 `SHOW CREATE TABLE` 的建表语句。
pub async fn show_create_table(
    conn: &mut mysql_async::Conn,
    schema: &str,
    table: &str,
) -> Result<String> {
    let sql = format!(
        "SHOW CREATE TABLE {}.{}",
        quote_ident(schema),
        quote_ident(table)
    );
    let row: Option<(String, String)> = conn.query_first(sql).await?;
    row.map(|(_, ddl)| ddl)
        .ok_or_else(|| Error::Other(format!("读取 {schema}.{table} 的建表语句失败")))
}

/// 读取服务端版本号，如 `8.0.36`。
pub async fn server_version(conn: &mut mysql_async::Conn) -> Result<(u16, u16, u16)> {
    let v: Option<String> = conn.query_first("SELECT VERSION()").await?;
    let v = v.unwrap_or_default();
    Ok(parse_version(&v))
}

/// 解析版本串。兼容 `8.0.36`、`5.7.44-log`、`10.11.6-MariaDB`。
pub fn parse_version(v: &str) -> (u16, u16, u16) {
    let mut parts = v.split(|c: char| c == '.' || c == '-');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

pub async fn is_mariadb(conn: &mut mysql_async::Conn) -> Result<bool> {
    let v: Option<String> = conn.query_first("SELECT VERSION()").await?;
    Ok(v.unwrap_or_default().to_ascii_lowercase().contains("mariadb"))
}

/// 读取 max_allowed_packet（字节）。
pub async fn max_allowed_packet(conn: &mut mysql_async::Conn) -> Result<u64> {
    let row: Option<(String, String)> = conn
        .query_first("SHOW VARIABLES LIKE 'max_allowed_packet'")
        .await?;
    Ok(row
        .and_then(|(_, v)| v.parse::<u64>().ok())
        .unwrap_or(4 * 1024 * 1024))
}

/// 读取表的估算行数（information_schema.TABLES.TABLE_ROWS，InnoDB 上是估算值）。
pub async fn estimate_rows(conn: &mut mysql_async::Conn, schema: &str, table: &str) -> Result<u64> {
    let row: Option<(Option<u64>,)> = conn
        .exec_first(
            "SELECT TABLE_ROWS FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
            (schema, table),
        )
        .await?;
    Ok(row.and_then(|(r,)| r).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("8.0.36"), (8, 0, 36));
        assert_eq!(parse_version("5.7.44-log"), (5, 7, 44));
        assert_eq!(parse_version("10.11.6-MariaDB"), (10, 11, 6));
        assert_eq!(parse_version("garbage"), (0, 0, 0));
    }
}
