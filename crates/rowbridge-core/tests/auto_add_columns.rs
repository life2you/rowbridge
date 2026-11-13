//! 目标表缺列时自动补列的集成测试。
//!
//! 需要本机 e2e 容器（3307 源 / 3308 目标）：
//! `cargo test -p rowbridge-core --test auto_add_columns -- --nocapture`
//!
//! 场景对应线上常见故障：源表后来加了列，目标表还是老结构，
//! 预检只给 warn 就放行，写入时报 `1054 Unknown column ... in 'field list'`。

use mysql_async::prelude::*;
use rowbridge_core::config::{ConnConfig, TableTaskConfig, TaskConfig, TaskOptions};
use rowbridge_core::engine::{self, Event, EventSink};
use rowbridge_core::preflight;
use tokio_util::sync::CancellationToken;

fn conn(port: u16) -> ConnConfig {
    ConnConfig {
        host: "127.0.0.1".into(),
        port,
        username: "root".into(),
        password: "root123".into(),
        schema: None,
        use_tls: false,
    }
}

fn src_sql(schema: &str) -> String {
    format!(
        "DROP DATABASE IF EXISTS `{schema}`;
         CREATE DATABASE `{schema}`;
         CREATE TABLE `{schema}`.t (
            id INT NOT NULL,
            name VARCHAR(20) NOT NULL,
            extra INT NULL,
            note VARCHAR(10) NULL,
            PRIMARY KEY (id)
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
         INSERT INTO `{schema}`.t VALUES (1,'a',10,'x'), (2,'b',20,NULL), (3,'c',30,'z');"
    )
}

fn dst_sql(schema: &str) -> String {
    format!(
        "DROP DATABASE IF EXISTS `{schema}`;
         CREATE DATABASE `{schema}`;
         CREATE TABLE `{schema}`.t (
            id INT NOT NULL,
            name VARCHAR(20) NOT NULL,
            PRIMARY KEY (id)
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;"
    )
}

fn table(src_db: &str, dst_db: &str) -> TableTaskConfig {
    TableTaskConfig {
        table: "t".into(),
        src_schema: src_db.into(),
        dst_schema: dst_db.into(),
        dst_table: None,
        columns: Vec::new(),
        cursor_start: None,
        start_id: None,
        end_id: None,
        truncate_target: None,
        create_table: None,
        continue_on_error: None,
        strip_auto_increment: None,
        strip_foreign_keys: None,
        map_collation: None,
        filter: None,
    }
}

async fn setup(src_db: &str, dst_db: &str) -> bool {
    let Ok(mut s) = mysql_async::Conn::new(conn(3307).to_url().unwrap()).await else {
        return false;
    };
    let Ok(mut d) = mysql_async::Conn::new(conn(3308).to_url().unwrap()).await else {
        return false;
    };
    s.query_drop(src_sql(src_db)).await.unwrap();
    d.query_drop(dst_sql(dst_db)).await.unwrap();
    let _ = s.disconnect().await;
    let _ = d.disconnect().await;
    true
}

async fn target_columns(dst_db: &str) -> Vec<String> {
    let mut d = mysql_async::Conn::new(conn(3308).to_url().unwrap())
        .await
        .unwrap();
    let rows: Vec<String> = d
        .exec(
            "SELECT COLUMN_NAME FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA=? AND TABLE_NAME='t' ORDER BY ORDINAL_POSITION",
            (dst_db,),
        )
        .await
        .unwrap();
    let _ = d.disconnect().await;
    rows
}

async fn run_with(
    src_db: &str,
    dst_db: &str,
    add_missing_columns: bool,
) -> (Vec<String>, u64, Option<String>) {
    let tables = vec![table(src_db, dst_db)];
    let options = TaskOptions {
        parallelism: 1,
        add_missing_columns,
        ..Default::default()
    };
    let mut sc = mysql_async::Conn::new(conn(3307).to_url().unwrap())
        .await
        .unwrap();
    let mut dc = mysql_async::Conn::new(conn(3308).to_url().unwrap())
        .await
        .unwrap();
    let report = preflight::run(&mut sc, &mut dc, &tables, &options, false)
        .await
        .expect("预检失败");
    assert!(
        !report.any_blocking_error(),
        "缺列不应阻塞预检：{:?}",
        report.tables[0].checks
    );
    assert!(
        report.tables[0]
            .checks
            .iter()
            .any(|c| c.code == "column_missing_in_target"),
        "预检应提示目标缺列"
    );
    assert_eq!(report.tables[0].alter_sqls.len(), 2, "应给出 2 条补列 DDL");

    let cfg = TaskConfig {
        source: conn(3307),
        target: conn(3308),
        tables,
        options,
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sink = EventSink::new(tx);

    let result = engine::run_task(
        cfg,
        &report,
        sink,
        CancellationToken::new(),
        engine::PauseController::default(),
    )
    .await;

    let mut logs = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let Event::Log { level, message, .. } = e {
            logs.push(format!("{level}: {message}"));
        }
    }

    let (rows, err) = match result {
        Ok(r) => (
            r.tables.iter().map(|t| t.rows).sum::<u64>(),
            r.tables
                .iter()
                .find_map(|t| t.error.clone())
                .or(if r.failed > 0 {
                    Some(format!("{} 张表失败", r.failed))
                } else {
                    None
                }),
        ),
        Err(e) => (0, Some(e.to_string())),
    };
    (logs, rows, err)
}

#[tokio::test]
async fn missing_columns_are_added_automatically() {
    const SRC_DB: &str = "db_rb_addcol_src_a";
    const DST_DB: &str = "db_rb_addcol_dst_a";
    if !setup(SRC_DB, DST_DB).await {
        eprintln!("跳过：e2e 容器未启动");
        return;
    }

    let (logs, rows, err) = run_with(SRC_DB, DST_DB, true).await;
    assert_eq!(err, None, "同步应成功，日志：{logs:#?}");
    assert_eq!(rows, 3, "应同步 3 行");

    let cols = target_columns(DST_DB).await;
    assert_eq!(
        cols,
        vec!["id", "name", "extra", "note"],
        "目标表应补出缺失列且顺序与源表一致"
    );
    assert!(
        logs.iter().any(|l| l.contains("已自动补列")),
        "日志应说明自动补列，实际：{logs:#?}"
    );

    // 补出来的列要真的写了数据
    let mut d = mysql_async::Conn::new(conn(3308).to_url().unwrap())
        .await
        .unwrap();
    let v: Option<(Option<i32>, Option<String>)> = d
        .exec_first(
            "SELECT extra, note FROM `db_rb_addcol_dst_a`.t WHERE id = 1",
            (),
        )
        .await
        .unwrap();
    assert_eq!(v, Some((Some(10), Some("x".to_string()))));
    let _ = d.disconnect().await;
}

#[tokio::test]
async fn missing_columns_respect_switch() {
    const SRC_DB: &str = "db_rb_addcol_src_b";
    const DST_DB: &str = "db_rb_addcol_dst_b";
    if !setup(SRC_DB, DST_DB).await {
        eprintln!("跳过：e2e 容器未启动");
        return;
    }

    let (logs, _rows, err) = run_with(SRC_DB, DST_DB, false).await;
    assert!(err.is_some(), "关闭自动补列后写入应失败");
    let msg = err.unwrap();
    assert!(
        msg.contains("1054") || msg.contains("Unknown column"),
        "错误应指向未知列，实际：{msg}"
    );
    assert!(
        msg.contains("db_rb_addcol_dst_b.t"),
        "错误应标明是目标表，实际：{msg}"
    );
    assert!(
        logs.iter().any(|l| l.contains("未开启")),
        "应给出未开启自动补列的告警，实际：{logs:#?}"
    );
}
