//! RowBridge 命令行入口。
//!
//! 引擎可以脱离 UI 单独跑，支持命令行人机交互，
//! 也支持通过 --json 供脚本或调用方结构化解析。

use clap::{Parser, Subcommand};
use rowbridge_core::config::{ConnConfig, CursorStart, TableTaskConfig, TaskConfig, TaskOptions};
use rowbridge_core::engine::{self, Event, EventSink};
use rowbridge_core::{connections, introspect, preflight};
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(name = "rowbridge-cli", version, about = "RowBridge 同步引擎 CLI")]
struct Cli {
    /// 机器友好的纯 JSON 输出（适合 Agent 或脚本调用）
    #[arg(long, global = true)]
    json: bool,

    /// 已保存的源数据源名称或 ID（优先于 --src-host 等）
    #[arg(long)]
    src_conn: Option<String>,
    #[arg(long, default_value = "127.0.0.1")]
    src_host: String,
    #[arg(long, default_value_t = 3306)]
    src_port: u16,
    #[arg(long, default_value = "root")]
    src_user: String,
    #[arg(long, default_value = "")]
    src_pass: String,

    /// 已保存的目标数据源名称或 ID（优先于 --dst-host 等）
    #[arg(long)]
    dst_conn: Option<String>,
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
    /// 列出所有已保存的数据源
    Connections,
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
    /// 仅同步表结构（自动建库、建表、补齐新增列，不搬运数据）
    Schema {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long = "table", required = true)]
        tables: Vec<String>,
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
        /// 同步前清空目标表已有数据 (TRUNCATE TABLE)
        #[arg(long)]
        truncate: bool,
        #[arg(long)]
        end_id: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_adaptive: bool,
        #[arg(long)]
        disable_foreign_key_checks: bool,
        #[arg(long)]
        relax_sql_mode: bool,
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

    if matches!(cli.cmd, Cmd::Connections) {
        let conns = connections::list().unwrap_or_default();
        if cli.json {
            let summaries: Vec<_> = conns.iter().map(|c| c.safe_summary()).collect();
            println!("{}", serde_json::to_string_pretty(&summaries)?);
        } else {
            for c in conns {
                println!("{}\t{}\t{}:{}\t{}", c.id, c.name, c.host, c.port, c.username);
            }
        }
        return Ok(());
    }

    let src = if let Some(name) = &cli.src_conn {
        connections::find(name)
            .ok_or_else(|| format!("未找到名为 '{name}' 的源数据源配置"))?
            .to_conn_config(None)
    } else {
        conn(&cli.src_host, cli.src_port, &cli.src_user, &cli.src_pass, None)
    };

    let dst = if let Some(name) = &cli.dst_conn {
        connections::find(name)
            .ok_or_else(|| format!("未找到名为 '{name}' 的目标数据源配置"))?
            .to_conn_config(None)
    } else {
        conn(&cli.dst_host, cli.dst_port, &cli.dst_user, &cli.dst_pass, None)
    };

    match cli.cmd {
        Cmd::Connections => unreachable!(),
        Cmd::Schemas => {
            let mut c = mysql_async::Conn::new(src.to_url()?).await?;
            let schemas = introspect::list_schemas(&mut c).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&schemas)?);
            } else {
                for s in schemas {
                    println!("{}\t{}", s.name, s.collation.unwrap_or_default());
                }
            }
        }
        Cmd::Tables { schema } => {
            let mut c = mysql_async::Conn::new(src.to_url()?).await?;
            let tables = introspect::list_tables(&mut c, &schema).await?;
            let pk_rows = introspect::list_primary_keys(&mut c, &schema).await?;
            let mut pk_map: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for (t, col, _seq, _non_unique) in pk_rows {
                pk_map.entry(t).or_default().push(col);
            }

            if cli.json {
                let out: Vec<_> = tables
                    .iter()
                    .map(|t| {
                        let is_view = t.table_type.as_deref().map(|ty| ty.eq_ignore_ascii_case("VIEW")).unwrap_or(false);
                        let pk = pk_map.get(&t.name).cloned();
                        let syncable = !is_view && pk.as_ref().map(|p| p.len() == 1).unwrap_or(false);
                        serde_json::json!({
                            "name": t.name,
                            "rows_estimate": t.rows_estimate.unwrap_or(0),
                            "engine": t.engine,
                            "primary_key": pk.map(|p| p.join(", ")),
                            "is_view": is_view,
                            "syncable": syncable,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                for t in tables {
                    println!(
                        "{}\t{}\t{}",
                        t.name,
                        t.rows_estimate.unwrap_or(0),
                        t.engine.unwrap_or_default()
                    );
                }
            }
        }
        Cmd::Describe { schema, table } => {
            let mut c = mysql_async::Conn::new(src.to_url()?).await?;
            let d = introspect::table_detail(&mut c, &schema, &table).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&d)?);
            } else {
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
        }
        Cmd::Schema { from, to, tables } => {
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
                    truncate_target: None,
                })
                .collect();
            let options = TaskOptions::default();
            let mut sc = mysql_async::Conn::new(src.to_url()?).await?;
            let mut dc = mysql_async::Conn::new(dst.to_url()?).await?;
            let same = preflight::same_instance(&src, &dst);
            let report = preflight::run(&mut sc, &mut dc, &tasks, &options, same).await?;
            let cfg = TaskConfig {
                source: src,
                target: dst,
                tables: tasks,
                options,
            };
            let res = engine::sync_schema(&cfg, &report).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!(
                    "表结构同步完成 ({}ms): 建库 {} 个, 建表 {} 个, 补列 {} 处, 错误 {} 个",
                    res.elapsed_ms,
                    res.databases_created.len(),
                    res.tables_created.len(),
                    res.columns_added.len(),
                    res.errors.len(),
                );
                for db in &res.databases_created {
                    println!("  [+] 数据库创建: {db}");
                }
                for tbl in &res.tables_created {
                    println!("  [+] 表创建: {tbl}");
                }
                for col in &res.columns_added {
                    println!("  [+] 列补齐: {col}");
                }
                for err in &res.errors {
                    eprintln!("  [-] 失败: {err}");
                }
            }
        }
        Cmd::Sync {
            from,
            to,
            tables,
            batch,
            parallel,
            cursor,
            truncate,
            end_id,
            dry_run,
            no_adaptive,
            disable_foreign_key_checks,
            relax_sql_mode,
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
                    truncate_target: None,
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
                truncate_target: truncate,
                end_id,
                disable_foreign_key_checks,
                relax_sql_mode,
                continue_on_error: true,
                ..Default::default()
            };

            let mut sc = mysql_async::Conn::new(src.to_url()?).await?;
            let mut dc = mysql_async::Conn::new(dst.to_url()?).await?;
            let same = preflight::same_instance(&src, &dst);
            let report = preflight::run(&mut sc, &mut dc, &tasks, &options, same).await?;

            if dry_run {
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
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
                    }
                    println!("\n(仅预检)");
                }
                return Ok(());
            }

            if report.any_blocking_error() {
                if cli.json {
                    eprintln!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("\n存在阻塞项，终止。");
                }
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

            // Ctrl-C 优雅取消
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

            let is_json = cli.json;
            let printer = tokio::spawn(async move {
                while let Some(e) = rx.recv().await {
                    if is_json {
                        continue;
                    }
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

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
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
    }

    Ok(())
}
