//! RowBridge 命令行入口。
//!
//! 引擎先在这里验证正确性，UI 只是它的外壳。

use clap::{Parser, Subcommand};
use rowbridge_core::config::{ConnConfig, CursorStart, TableTaskConfig, TaskConfig, TaskOptions};
use rowbridge_core::engine::{self, Event, EventSink};
use rowbridge_core::{introspect, preflight};
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(name = "rowbridge-cli", version, about = "RowBridge 同步引擎 CLI")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    src_host: String,
    #[arg(long, default_value_t = 3306)]
    src_port: u16,
    #[arg(long, default_value = "root")]
    src_user: String,
    #[arg(long, default_value = "")]
    src_pass: String,

    #[arg(long, default_value = "127.0.0.1")]
    dst_host: String,
    #[arg(long, default_value_t = 3306)]
    dst_port: u16,
    #[arg(long, default_value = "root")]
    dst_user: String,
    #[arg(long, default_value = "")]
    dst_pass: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// 列出源连接上的库
    Schemas,
    /// 列出某个库的表
    Tables {
        #[arg(long)]
        schema: String,
    },
    /// 查看表结构
    Describe {
        #[arg(long)]
        schema: String,
        #[arg(long)]
        table: String,
    },
    /// 预检 + 同步
    Sync {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long = "table", required = true)]
        tables: Vec<String>,
        #[arg(long, default_value_t = 1000)]
        batch: usize,
        #[arg(long, default_value_t = 1)]
        parallel: usize,
        /// 游标起点: full | resume
        #[arg(long, default_value = "full")]
        cursor: String,
        #[arg(long)]
        end_id: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_adaptive: bool,
    },
}

fn conn(host: &str, port: u16, user: &str, pass: &str, schema: Option<String>) -> ConnConfig {
    ConnConfig {
        host: host.to_string(),
        port,
        username: user.to_string(),
        password: pass.to_string(),
        schema,
        use_tls: false,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let src = conn(&cli.src_host, cli.src_port, &cli.src_user, &cli.src_pass, None);
    let dst = conn(&cli.dst_host, cli.dst_port, &cli.dst_user, &cli.dst_pass, None);

    match cli.cmd {
        Cmd::Schemas => {
            let mut c = mysql_async::Conn::new(src.to_url()?).await?;
            for s in introspect::list_schemas(&mut c).await? {
                println!("{}\t{}", s.name, s.collation.unwrap_or_default());
            }
        }
        Cmd::Tables { schema } => {
            let mut c = mysql_async::Conn::new(src.to_url()?).await?;
            for t in introspect::list_tables(&mut c, &schema).await? {
                println!(
                    "{}\t{}\t{}",
                    t.name,
                    t.rows_estimate.unwrap_or(0),
                    t.engine.unwrap_or_default()
                );
            }
        }
        Cmd::Describe { schema, table } => {
            let mut c = mysql_async::Conn::new(src.to_url()?).await?;
            let d = introspect::table_detail(&mut c, &schema, &table).await?;
            println!("PK: {:?}", d.primary_key);
            for col in &d.columns {
                println!(
                    "  {} {} nullable={} extra={}",
                    col.name, col.column_type, col.is_nullable, col.extra
                );
            }
            match d.pk_column() {
                Ok(c) => println!("可同步，主键 {}", c.name),
                Err(e) => println!("不可同步: {e}"),
            }
        }
        Cmd::Sync {
            from,
            to,
            tables,
            batch,
            parallel,
            cursor,
            end_id,
            dry_run,
            no_adaptive,
        } => {
            let tasks: Vec<TableTaskConfig> = tables
                .iter()
                .map(|t| TableTaskConfig {
                    table: t.clone(),
                    src_schema: from.clone(),
                    dst_schema: to.clone(),
                    dst_table: None,
                    columns: Vec::new(),
                    cursor_start: None,
                    start_id: None,
                    end_id: None,
                })
                .collect();

            let options = TaskOptions {
                parallelism: parallel,
                batch_size: batch,
                adaptive_batch: !no_adaptive,
                cursor_start: if cursor == "resume" {
                    CursorStart::Resume
                } else {
                    CursorStart::Full
                },
                end_id,
                ..Default::default()
            };

            let mut sc = mysql_async::Conn::new(src.to_url()?).await?;
            let mut dc = mysql_async::Conn::new(dst.to_url()?).await?;
            let same = preflight::same_instance(&src, &dst);
            let report = preflight::run(&mut sc, &mut dc, &tasks, &options, same).await?;

            println!(
                "源版本 {} / 目标版本 {}",
                report.source_version, report.target_version
            );
            for t in &report.tables {
                println!(
                    "\n[{}{}] {}.{} → {}.{}  {} 行(估算)  max_pk={:?} → {:?}",
                    if t.syncable { "OK " } else { "SKIP " },
                    if t.needs_create_table {
                        "需建表"
                    } else {
                        "表已存在"
                    },
                    t.source_schema,
                    t.table,
                    t.target_schema,
                    t.target_table,
                    t.source_rows_estimate,
                    t.source_max_pk,
                    t.target_max_pk
                );
                for c in &t.checks {
                    println!("    [{:?}] {}", c.severity, c.message);
                }
                if let Some(ct) = &t.create_table {
                    println!("    --- 建表 DDL ---");
                    for line in ct.ddl.lines() {
                        println!("    {line}");
                    }
                }
            }

            if dry_run {
                println!("\n(仅预检)");
                return Ok(());
            }
            if report.any_blocking_error() {
                println!("\n存在阻塞项，终止。");
                std::process::exit(1);
            }

            let cfg = TaskConfig {
                source: src,
                target: dst,
                tables: tasks,
                options,
            };

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let sink = EventSink::new(tx);
            let cancel = CancellationToken::new();
            // Ctrl-C 优雅取消：在批边界停下，已完成的批次保持有效。
            {
                let cancel = cancel.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        eprintln!("\n收到中断信号，正在停止…（再按一次强制退出）");
                        cancel.cancel();
                        if tokio::signal::ctrl_c().await.is_ok() {
                            std::process::exit(130);
                        }
                    }
                });
            }

            let printer = tokio::spawn(async move {
                while let Some(e) = rx.recv().await {
                    match e {
                        Event::BatchDone {
                            table,
                            total_rows,
                            cursor,
                            rows_per_sec,
                            ..
                        } => {
                            println!(
                                "  {table}: {total_rows} 行, 游标 {cursor}, {rows_per_sec:.0} 行/s"
                            );
                        }
                        Event::TableDone {
                            table,
                            rows,
                            elapsed_ms,
                        } => {
                            println!("✓ {table} 完成 {rows} 行, {elapsed_ms}ms");
                        }
                        Event::TableFailed { table, error } => {
                            println!("✗ {table} 失败: {error}");
                        }
                        Event::TableCreated { table, .. } => {
                            println!("+ 已创建目标表 {table}");
                        }
                        Event::Warning { table, message } => {
                            println!("! {table:?} {message}");
                        }
                        Event::TaskDone {
                            total_rows,
                            succeeded,
                            failed,
                            elapsed_ms,
                        } => {
                            println!(
                                "\n任务完成: {total_rows} 行, 成功 {succeeded} 表, 失败 {failed} 表, {elapsed_ms}ms"
                            );
                        }
                        Event::Cancelled => println!("\n已取消"),
                        Event::Log {
                            table,
                            level,
                            message,
                        } => {
                            println!("[{level}] {table:?} {message}");
                        }
                        _ => {}
                    }
                }
            });

            let result = engine::run_task(cfg, &report, sink, cancel).await?;
            let _ = printer.await;
            println!(
                "\n最终: {} 行, {} 成功, {} 失败, {} 未完成(已取消), {}ms",
                result.total_rows,
                result.succeeded,
                result.failed,
                result.partial,
                result.elapsed_ms
            );
        }
    }

    Ok(())
}
