//! 单表多线程分片并行的集成测试。
//!
//! 需要本机 e2e 容器（3307 源 / 3308 目标）：
//! `cargo test -p rowbridge-core --test parallel_table -- --nocapture`

use mysql_async::prelude::*;
use rowbridge_core::config::{ConnConfig, CursorStart, TableTaskConfig, TaskConfig, TaskOptions};
use rowbridge_core::engine::{self, Event, EventSink};
use rowbridge_core::preflight;
use tokio_util::sync::CancellationToken;

fn src() -> ConnConfig {
    ConnConfig {
        host: "127.0.0.1".into(),
        port: 3307,
        username: "root".into(),
        password: "root123".into(),
        schema: None,
        use_tls: false,
    }
}

fn dst() -> ConnConfig {
    ConnConfig {
        host: "127.0.0.1".into(),
        port: 3308,
        username: "root".into(),
        password: "root123".into(),
        schema: None,
        use_tls: false,
    }
}

async fn prepare(schema: &str) -> bool {
    let Ok(mut c) = mysql_async::Conn::new(dst().to_url().unwrap()).await else {
        return false;
    };
    let _ = c
        .query_drop(format!(
            "DROP DATABASE IF EXISTS `{schema}`; CREATE DATABASE `{schema}`"
        ))
        .await;
    let _ = c.disconnect().await;
    true
}

fn make_table(
    dst_schema: &str,
    table: &str,
    cursor: Option<CursorStart>,
    start: Option<&str>,
    end: Option<&str>,
    tp: Option<usize>,
) -> TableTaskConfig {
    TableTaskConfig {
        table: table.into(),
        src_schema: "db_a".into(),
        dst_schema: dst_schema.into(),
        dst_table: None,
        columns: Vec::new(),
        cursor_start: cursor,
        start_id: start.map(str::to_string),
        end_id: end.map(str::to_string),
        truncate_target: None,
        create_table: None,
        continue_on_error: None,
        strip_auto_increment: None,
        strip_foreign_keys: None,
        map_collation: None,
        filter: None,
        table_parallelism: tp,
    }
}

#[tokio::test]
async fn parallel_shards_split_1_to_10000_into_4_threads() {
    const DB: &str = "db_rb_parallel_a";
    if !prepare(DB).await {
        eprintln!("跳过：e2e 容器未启动");
        return;
    }

    let tables = vec![make_table(
        DB,
        "bulk",
        Some(CursorStart::Custom),
        Some("1"),
        Some("10000"),
        Some(4),
    )];
    let options = TaskOptions {
        parallelism: 1,
        table_parallelism: 1,
        batch_size: 400,
        adaptive_batch: false,
        ..Default::default()
    };

    let mut sc = mysql_async::Conn::new(src().to_url().unwrap()).await.unwrap();
    let mut dc = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
    let report = preflight::run(&mut sc, &mut dc, &tables, &options, false)
        .await
        .expect("预检失败");
    assert_eq!(report.tables[0].table_parallelism, 4);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cfg = TaskConfig {
        source: src(),
        target: dst(),
        tables,
        options,
    };

    let res = engine::run_task(
        cfg,
        &report,
        EventSink::new(tx),
        CancellationToken::new(),
        engine::PauseController::default(),
    )
    .await
    .expect("同步失败");
    assert_eq!(res.failed, 0);
    assert_eq!(res.total_rows, 10_000);

    let mut shard_log = None;
    let mut planned_shards = Vec::new();
    let mut shard_done_rows = std::collections::HashMap::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Event::Log { message, .. } if message.contains("并行分片") => {
                shard_log = Some(message);
            }
            Event::TableShardsPlanned { shards, .. } => {
                planned_shards = shards;
            }
            Event::ShardDone {
                shard_index, rows, ..
            } => {
                shard_done_rows.insert(shard_index, rows);
            }
            _ => {}
        }
    }
    assert_eq!(planned_shards.len(), 4, "应下发 4 个分片规划事件");
    assert_eq!(shard_done_rows.len(), 4, "应收到 4 个分片完成事件");
    assert_eq!(
        shard_done_rows.values().sum::<u64>(),
        10_000,
        "4 个分片完成行数之和应为 10000"
    );
    let log = shard_log.expect("应输出分片区间日志");
    assert!(log.contains("#1 [1..=2500]"), "分片1日志异常: {log}");
    assert!(log.contains("#2 [2501..=5000]"), "分片2日志异常: {log}");
    assert!(log.contains("#3 [5001..=7500]"), "分片3日志异常: {log}");
    assert!(log.contains("#4 [7501..=10000]"), "分片4日志异常: {log}");

    let mut sc = mysql_async::Conn::new(src().to_url().unwrap()).await.unwrap();
    let mut dc = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
    let src_sum: Option<i64> = sc
        .query_first("SELECT CAST(SUM(n) AS SIGNED) FROM db_a.bulk")
        .await
        .unwrap();
    let dst_count: Option<u64> = dc
        .query_first(format!("SELECT COUNT(*) FROM `{DB}`.bulk"))
        .await
        .unwrap();
    let dst_sum: Option<i64> = dc
        .query_first(format!("SELECT CAST(SUM(n) AS SIGNED) FROM `{DB}`.bulk"))
        .await
        .unwrap();
    assert_eq!(dst_count, Some(10_000));
    assert_eq!(dst_sum, src_sum);
}

#[tokio::test]
async fn parallel_shards_support_full_and_resume_modes() {
    const DB: &str = "db_rb_parallel_b";
    if !prepare(DB).await {
        eprintln!("跳过：e2e 容器未启动");
        return;
    }

    // 先按单线程写入 1..=2500
    let first_tables = vec![make_table(
        DB,
        "bulk",
        Some(CursorStart::Custom),
        Some("1"),
        Some("2500"),
        Some(1),
    )];
    let first_opts = TaskOptions {
        parallelism: 1,
        table_parallelism: 1,
        batch_size: 1000,
        adaptive_batch: false,
        ..Default::default()
    };

    let mut sc = mysql_async::Conn::new(src().to_url().unwrap()).await.unwrap();
    let mut dc = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
    let rep1 = preflight::run(&mut sc, &mut dc, &first_tables, &first_opts, false)
        .await
        .unwrap();
    let res1 = engine::run_task(
        TaskConfig {
            source: src(),
            target: dst(),
            tables: first_tables,
            options: first_opts,
        },
        &rep1,
        EventSink::silent(),
        CancellationToken::new(),
        engine::PauseController::default(),
    )
    .await
    .unwrap();
    assert_eq!(res1.total_rows, 2500);

    // 再用 Resume + 全局 4 线程续传剩下的 2501..=10000
    let resume_tables = vec![make_table(
        DB,
        "bulk",
        Some(CursorStart::Resume),
        None,
        None,
        None,
    )];
    let resume_opts = TaskOptions {
        parallelism: 1,
        table_parallelism: 4,
        batch_size: 500,
        adaptive_batch: true,
        ..Default::default()
    };
    let rep2 = preflight::run(&mut sc, &mut dc, &resume_tables, &resume_opts, false)
        .await
        .unwrap();
    assert!(
        rep2.tables[0]
            .checks
            .iter()
            .any(|c| c.code == "parallel_resume_caveat"),
        "续传+单表并发应给出提示警告"
    );

    let res2 = engine::run_task(
        TaskConfig {
            source: src(),
            target: dst(),
            tables: resume_tables,
            options: resume_opts,
        },
        &rep2,
        EventSink::silent(),
        CancellationToken::new(),
        engine::PauseController::default(),
    )
    .await
    .unwrap();
    assert_eq!(res2.failed, 0);
    assert_eq!(res2.total_rows, 7500);

    let dst_count: Option<u64> = dc
        .query_first(format!("SELECT COUNT(*) FROM `{DB}`.bulk"))
        .await
        .unwrap();
    assert_eq!(dst_count, Some(10_000));
}
