//! 逐表游标的集成测试。
//!
//! 需要本机 e2e 容器（3307 源 / 3308 目标）。
//! 用 `cargo test -p rowbridge-core --test per_table_cursor -- --nocapture` 运行，
//! 或在 `tests/e2e/run.sh` 里被调用。
//!
//! 这里测的是 CLI 覆盖不到的部分：同一批任务里，不同表用各自的游标区间。

use rowbridge_core::config::{
    ConnConfig, CursorStart, TableTaskConfig, TaskConfig, TaskOptions,
};
use rowbridge_core::engine::{self, EventSink};
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

fn task_in(
    dst_schema: &str,
    table: &str,
    cs: Option<CursorStart>,
    start: Option<&str>,
    end: Option<&str>,
) -> TableTaskConfig {
    TableTaskConfig {
        table: table.into(),
        src_schema: "db_a".into(),
        dst_schema: dst_schema.into(),
        dst_table: None,
        columns: Vec::new(),
        cursor_start: cs,
        start_id: start.map(str::to_string),
        end_id: end.map(str::to_string),
    }
}

async fn count(schema: &str, table: &str) -> u64 {
    use mysql_async::prelude::*;
    let mut c = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
    let n: Option<u64> = c
        .query_first(format!("SELECT COUNT(*) FROM `{schema}`.`{table}`"))
        .await
        .unwrap();
    let _ = c.disconnect().await;
    n.unwrap_or(0)
}

/// 每个测试用**自己独立的库**，否则并行跑会互相污染。
async fn prepare(schema: &str) -> bool {
    use mysql_async::prelude::*;
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

#[tokio::test]
async fn per_table_cursors_are_independent() {
    const DB: &str = "db_rb_cursor_a";
    if !prepare(DB).await {
        eprintln!("跳过：e2e 容器未启动");
        return;
    }

    // 三张表，三种不同的游标策略：
    //   bulk   自定义 1000 → 2000，应恰好 1000 行
    //   simple 自定义 id > 2，应恰好 3 行（3、4、5）
    //   empty_t 全量，应为 0 行
    let tables = vec![
        task_in(DB, "bulk", Some(CursorStart::Custom), Some("1000"), Some("2000")),
        task_in(DB, "simple", Some(CursorStart::Custom), Some("2"), None),
        task_in(DB, "empty_t", Some(CursorStart::Full), None, None),
    ];

    // 全局故意设成会「搞错」的值：如果引擎误用了全局游标，
    // bulk 就会从 5000 开始、simple 会从 5000 开始（一行都同步不到）。
    let options = TaskOptions {
        parallelism: 1,
        cursor_start: CursorStart::Custom,
        start_id: Some("5000".into()),
        end_id: None,
        adaptive_batch: false,
        batch_size: 300,
        ..Default::default()
    };

    let cfg = TaskConfig {
        source: src(),
        target: dst(),
        tables: tables.clone(),
        options: options.clone(),
    };

    let mut sc = mysql_async::Conn::new(src().to_url().unwrap()).await.unwrap();
    let mut dc = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
    let report = preflight::run(&mut sc, &mut dc, &tables, &options, false)
        .await
        .expect("预检失败");
    assert!(!report.any_blocking_error(), "预检有阻塞项");

    let result = engine::run_task(
        cfg,
        &report,
        EventSink::silent(),
        CancellationToken::new(),
    )
    .await
    .expect("任务失败");

    assert_eq!(result.failed, 0, "有表同步失败: {:?}", result.tables);

    // bulk: 1001..=2000
    assert_eq!(count(DB, "bulk").await, 1000, "bulk 区间不对");
    // simple: 3..=5
    assert_eq!(count(DB, "simple").await, 3, "simple 区间不对");
    // empty_t: 源表空
    assert_eq!(count(DB, "empty_t").await, 0, "empty_t 应为空");

    // 再确认边界值，确保不是「数量碰巧对上」
    {
        use mysql_async::prelude::*;
        let mut c = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
        let b: Option<(u64, u64)> = c
            .query_first(format!("SELECT MIN(id), MAX(id) FROM `{DB}`.bulk"))
            .await
            .unwrap();
        assert_eq!(b, Some((1001, 2000)), "bulk 边界不对");

        let s: Option<(u64, u64)> = c
            .query_first(format!("SELECT MIN(id), MAX(id) FROM `{DB}`.simple"))
            .await
            .unwrap();
        assert_eq!(s, Some((3, 5)), "simple 边界不对");
        let _ = c.disconnect().await;
    }
}

#[tokio::test]
async fn global_cursor_still_applies_when_table_has_no_override() {
    const DB: &str = "db_rb_cursor_b";
    if !prepare(DB).await {
        eprintln!("跳过：e2e 容器未启动");
        return;
    }

    // 全局设结束 id，表级什么都不设 → 应该继承全局的 200
    let tables = vec![task_in(DB, "bulk", None, None, None)];
    let options = TaskOptions {
        parallelism: 1,
        cursor_start: CursorStart::Full,
        end_id: Some("200".into()),
        adaptive_batch: false,
        batch_size: 100,
        ..Default::default()
    };

    let cfg = TaskConfig {
        source: src(),
        target: dst(),
        tables: tables.clone(),
        options: options.clone(),
    };

    let mut sc = mysql_async::Conn::new(src().to_url().unwrap()).await.unwrap();
    let mut dc = mysql_async::Conn::new(dst().to_url().unwrap()).await.unwrap();
    let report = preflight::run(&mut sc, &mut dc, &tables, &options, false)
        .await
        .expect("预检失败");

    engine::run_task(cfg, &report, EventSink::silent(), CancellationToken::new())
        .await
        .expect("任务失败");

    assert_eq!(count(DB, "bulk").await, 200, "应继承全局结束 id");
}
