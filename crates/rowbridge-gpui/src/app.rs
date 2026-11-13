//! RowBridge 主视图状态与动作（GPUI 版）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use rowbridge_core::config::{CursorStart, TableTaskConfig, TaskConfig, TaskOptions, WriteMode};
use rowbridge_core::connections::{self, SavedConnection};
use rowbridge_core::engine::Event;
use rowbridge_core::introspect::{ColumnInfo, SchemaInfo};
use rowbridge_core::preflight::PreflightReport;
use rowbridge_core::store;
use tokio_util::sync::CancellationToken;

use crate::model::*;
use crate::ops;

pub struct RowBridgeApp {
    pub(crate) src: ConnPanel,
    pub(crate) dst: ConnPanel,
    pub(crate) opt: OptionsPanel,
    pub(crate) saved: Vec<SavedConnection>,
    pub(crate) src_schemas: Vec<SchemaInfo>,
    pub(crate) dst_schemas: Vec<SchemaInfo>,
    /// 目标库列表是否已经从目标实例拉取过（区分「还没拉」和「真的一个库都没有」）
    pub(crate) dst_schemas_loaded: bool,
    pub(crate) src_schema: Option<String>,
    pub(crate) dst_schema: Option<String>,
    /// 目标端「应该」用的库名：默认跟源库同名。目标端还没这个库时
    /// `dst_schema` 保持 None（下拉框显示为空），这里记住待创建的库名。
    pub(crate) dst_schema_wanted: Option<String>,
    /// 预检通过后自动开始，用于「目标库不存在 → 确认创建 → 预检 → 开始」链路
    pub(crate) start_after_preflight: bool,
    pub(crate) tables: Vec<TableRow>,
    pub(crate) loading_tables: bool,

    /// 正在编辑覆盖参数的表名
    pub(crate) editing: Option<String>,
    pub(crate) ed_cursor: Option<CursorStart>,
    pub(crate) ed_truncate: Tri,
    pub(crate) ed_create_table: Tri,
    pub(crate) ed_strip_foreign_keys: Tri,
    pub(crate) ed_strip_auto_increment: Tri,
    pub(crate) ed_map_collation: Tri,
    pub(crate) ed_continue_on_error: Tri,
    #[allow(dead_code)]
    pub(crate) ed_dst_table: Entity<InputState>,
    pub(crate) ed_filter: Entity<InputState>,
    pub(crate) ed_start: Entity<InputState>,
    pub(crate) ed_end: Entity<InputState>,
    pub(crate) ed_columns: Vec<ColumnInfo>,
    pub(crate) loading_ed_columns: bool,
    pub(crate) table_columns_cache: HashMap<String, Vec<ColumnInfo>>,
    pub(crate) table_list_scroll: UniformListScrollHandle,

    pub(crate) tasks: Vec<SyncTask>,
    pub(crate) selected_task_id: Option<String>,

    pub(crate) report: Option<PreflightReport>,
    pub(crate) preflighting: bool,
    pub(crate) syncing_schema: bool,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) log_filter: Option<&'static str>,
    pub(crate) status: Option<(bool, SharedString)>,

    pub(crate) table_filter: Entity<InputState>,

    /// 数据源管理页：新建数据源表单
    pub(crate) adding_source: bool,
    pub(crate) new_source_name: Entity<InputState>,
    pub(crate) new_source_host: Entity<InputState>,
    pub(crate) new_source_port: Entity<InputState>,
    pub(crate) new_source_user: Entity<InputState>,
    pub(crate) new_source_password: Entity<InputState>,
    pub(crate) new_source_tls: bool,

    pub(crate) view: Tab,
    pub(crate) show_advanced: bool,
    pub(crate) smoke_done: bool,
    pub(crate) _subs: Vec<Subscription>,
}

impl RowBridgeApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = connections::init();
        let saved = connections::list().unwrap_or_default();
        let _ = store::init();
        let tasks = Self::load_history_tasks();

        let src = ConnPanel::new("用户名", window, cx);
        let dst = ConnPanel::new("用户名", window, cx);
        let opt = OptionsPanel::new(window, cx);
        let ed_dst_table =
            cx.new(|cx| InputState::new(window, cx).placeholder("目标表名，留空表示同名"));
        let ed_filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("过滤条件，如: status = 1 AND is_deleted = 0")
        });
        let ed_start = cx.new(|cx| InputState::new(window, cx).placeholder("起始 ID（含）"));
        let ed_end = cx.new(|cx| InputState::new(window, cx).placeholder("结束 ID（含）"));
        let table_filter = cx.new(|cx| InputState::new(window, cx).placeholder("搜索表名…"));

        let new_source_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("名称，如「生产只读」"));
        let new_source_host = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("主机")
                .default_value("127.0.0.1")
        });
        let new_source_port = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("端口")
                .default_value("3306")
        });
        let new_source_user = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("用户名")
                .default_value("root")
        });
        let new_source_password =
            cx.new(|cx| InputState::new(window, cx).placeholder("密码").masked(true));

        let mut subs = Vec::new();
        let watched: Vec<Entity<InputState>> = src
            .inputs()
            .into_iter()
            .chain(dst.inputs())
            .chain(opt.inputs())
            .chain([
                ed_dst_table.clone(),
                ed_filter.clone(),
                ed_start.clone(),
                ed_end.clone(),
                table_filter.clone(),
                new_source_name.clone(),
                new_source_host.clone(),
                new_source_port.clone(),
                new_source_user.clone(),
                new_source_password.clone(),
            ])
            .collect();
        for state in watched {
            subs.push(
                cx.subscribe_in(&state, window, |_, _, _: &InputEvent, _, cx| {
                    cx.notify();
                }),
            );
        }

        let mut app = Self {
            src,
            dst,
            opt,
            saved,
            src_schemas: Vec::new(),
            dst_schemas: Vec::new(),
            dst_schemas_loaded: false,
            src_schema: None,
            dst_schema: None,
            dst_schema_wanted: None,
            start_after_preflight: false,
            tables: Vec::new(),
            loading_tables: false,
            editing: None,
            ed_cursor: None,
            ed_truncate: Tri::Follow,
            ed_create_table: Tri::Follow,
            ed_strip_foreign_keys: Tri::Follow,
            ed_strip_auto_increment: Tri::Follow,
            ed_map_collation: Tri::Follow,
            ed_continue_on_error: Tri::Follow,
            ed_dst_table,
            ed_filter,
            ed_start,
            ed_end,
            ed_columns: Vec::new(),
            loading_ed_columns: false,
            table_columns_cache: HashMap::new(),
            table_list_scroll: UniformListScrollHandle::new(),
            tasks,
            selected_task_id: None,
            report: None,
            preflighting: false,
            syncing_schema: false,
            logs: Vec::new(),
            log_filter: None,
            status: None,
            table_filter,
            adding_source: false,
            new_source_name,
            new_source_host,
            new_source_port,
            new_source_user,
            new_source_password,
            new_source_tls: false,
            view: Tab::Sync,
            show_advanced: false,
            smoke_done: false,
            _subs: subs,
        };

        if let Some(first) = app.saved.first().cloned() {
            let id = first.id.clone();
            app.pick_saved(DbSide::Src, &id, window, cx);
            if let Some(second) = app.saved.get(1).cloned() {
                app.pick_saved(DbSide::Dst, &second.id, window, cx);
            } else {
                app.pick_saved(DbSide::Dst, &id, window, cx);
            }
        }

        app
    }

    /// 自检：`RB_GPUI_SMOKE=1` 时自动走一遍「选数据源 → 加载库 → 列表 → 预检」。
    /// 只读操作，不会写入任何数据。
    pub(crate) fn maybe_run_smoke(&self, window: &mut Window, cx: &mut Context<Self>) {
        if std::env::var("RB_GPUI_SMOKE").ok().as_deref() != Some("1") {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {

            let saved = this
                .update(cx, |this, _| this.saved.clone())
                .unwrap_or_default();
            let Some(conn) = saved.first().cloned() else {
                println!("[smoke] 没有已保存的数据源");
                return;
            };
            println!("[smoke] 使用数据源 {} ({}:{})", conn.name, conn.host, conn.port);

            let src_id = conn.id.clone();
            let dst_id = conn.id.clone();
            let _ = this.update_in(cx, |this, window, cx| {
                this.pick_saved(DbSide::Src, &src_id, window, cx);
                this.pick_saved(DbSide::Dst, &dst_id, window, cx);
            });
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2500))
                .await;

            let schemas = this
                .update(cx, |this, _| this.src_schemas.clone())
                .unwrap_or_default();
            println!("[smoke] 源库列表 {} 个: {:?}", schemas.len(), schemas.iter().take(5).map(|s| s.name.clone()).collect::<Vec<_>>());
            if schemas.is_empty() {
                println!("[smoke] 源库列表为空，退出");
                return;
            }

            let target = schemas
                .iter()
                .find(|s| s.name == "fbb")
                .map(|s| s.name.clone())
                .unwrap_or_else(|| schemas[0].name.clone());
            println!("[smoke] 选择源库 {target}");
            let _ = this.update_in(cx, |this, _, cx| {
                this.choose_schema(DbSide::Src, &target, cx);
                this.choose_schema(DbSide::Dst, &target, cx);
            });
            cx.background_executor()
                .timer(std::time::Duration::from_millis(3000))
                .await;

            let picked = this
                .update(cx, |this, _| {
                    let name = this
                        .tables
                        .iter()
                        .find(|t| t.unsupported.is_none())
                        .map(|t| t.name.clone());
                    let total = this.tables.len();
                    if let Some(name) = &name {
                        this.toggle_table(name, true);
                    }
                    (total, name)
                })
                .unwrap_or((0, None));
            println!("[smoke] 表清单 {} 张，勾选 {:?}", picked.0, picked.1);
            if picked.1.is_none() {
                println!("[smoke] 没有可同步的表，退出");
                return;
            }

            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(name) = this.tables.iter().find(|t| t.selected).map(|t| t.name.clone()) {
                    this.start_edit(&name, window, cx);
                    this.ed_cursor = Some(CursorStart::Custom);
                    set_input(&this.ed_start, "1", window, cx);
                    set_input(&this.ed_end, "500", window, cx);
                    set_input(&this.ed_filter, "status = 1 AND is_deleted = 0", window, cx);
                    this.apply_edit(cx);
                    println!("[smoke] 表级覆盖已写入: {:?}", this.tables.iter().find(|t| t.name == name).map(|t| t.over.clone()));
                }
            });
            let _ = this.update(cx, |this, cx| this.do_preflight(cx));
            cx.background_executor()
                .timer(std::time::Duration::from_millis(6000))
                .await;

            let summary = this
                .update(cx, |this, _| {
                    (
                        this.preflighting,
                        this.report.as_ref().map(|r| {
                            r.tables
                                .iter()
                                .map(|t| {
                                    format!(
                                        "{} syncable={} planned={} src_max={:?} dst_max={:?} create={} alter={}",
                                        t.table,
                                        t.syncable,
                                        t.planned_rows,
                                        t.source_max_pk,
                                        t.target_max_pk,
                                        t.needs_create_table,
                                        t.alter_sqls.len()
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                        this.status.clone().map(|(ok, msg)| format!("{ok} {msg}")),
                    )
                })
                .unwrap_or((false, None, None));
            println!("[smoke] 预检中={} 状态={:?}", summary.0, summary.2);
            match summary.1 {
                Some(rows) => {
                    for r in rows {
                        println!("[smoke]   {r}");
                    }
                }
                None => println!("[smoke] 没有预检报告"),
            }
            println!("[smoke] 完成");
        })
        .detach();
    }

    /// 自检：`RB_GPUI_SMOKE_DST=1` 时验证「目标库跟随源库同名 / 缺库留空 / 库名不同告警」。
    /// 全程只读：不建库、不写数据。
    pub(crate) fn maybe_run_dst_smoke(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = std::env::var("RB_GPUI_SMOKE_DST").unwrap_or_default();
        if !matches!(mode.as_str(), "1" | "2" | "3") {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            macro_rules! dump {
                ($tag:expr) => {{
                    let info = this
                        .update(cx, |this, _| {
                            (
                                this.src_schema.clone(),
                                this.dst_schema.clone(),
                                this.dst_schema_wanted.clone(),
                                this.missing_target_schema(),
                                this.schema_names_differ(),
                                this.dst_schemas.len(),
                            )
                        })
                        .unwrap_or_default();
                    eprintln!(
                        "[dst-smoke] {} | 源库={:?} 目标库={:?} 目标端待建={:?} 缺库={:?} 库名不同={} 目标端库数={}",
                        $tag, info.0, info.1, info.2, info.3, info.4, info.5
                    );
                }};
            }

            let saved = this.update(cx, |this, _| this.saved.clone()).unwrap_or_default();
            if saved.len() < 2 {
                eprintln!("[dst-smoke] 需要至少两个已保存数据源，退出");
                return;
            }
            let (src_conn, dst_conn) = (saved[0].clone(), saved[1].clone());
            eprintln!(
                "[dst-smoke] 源数据源 {} / 目标数据源 {}",
                src_conn.name, dst_conn.name
            );
            let _ = this.update_in(cx, |this, window, cx| {
                this.pick_saved(DbSide::Src, &src_conn.id, window, cx);
                this.pick_saved(DbSide::Dst, &dst_conn.id, window, cx);
            });
            cx.background_executor()
                .timer(std::time::Duration::from_millis(3500))
                .await;
            dump!("① 选好两侧数据源后");

            // ② 选一个目标端没有的源库：目标库应为空，并记为待创建
            let missing = this
                .update(cx, |this, _| {
                    this.src_schemas
                        .iter()
                        .map(|s| s.name.clone())
                        .find(|n| !this.dst_schemas.iter().any(|d| &d.name == n))
                })
                .ok()
                .flatten();
            if let Some(name) = missing {
                eprintln!("[dst-smoke] 选一个目标端没有的源库：{name}");
                let _ = this.update(cx, |this, cx| this.choose_schema(DbSide::Src, &name, cx));
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(4000))
                    .await;
                dump!("② 源库在目标端不存在");
            } else {
                eprintln!("[dst-smoke] 所有源库在目标端都存在，跳过②");
            }

            // ②s 停在「目标库待创建」这一步，交给人点「加入任务并开始」（RB_GPUI_SMOKE_DST=3）
            if std::env::var("RB_GPUI_SMOKE_DST").ok().as_deref() == Some("3") {
                let _ = this.update(cx, |this, cx| {
                    let pick = this
                        .tables
                        .iter()
                        .filter(|t| t.unsupported.is_none())
                        .min_by_key(|t| t.rows_estimate)
                        .map(|t| (t.name.clone(), t.rows_estimate));
                    if let Some((name, est)) = pick {
                        this.toggle_table(&name, true);
                        if let Some(row) = this.tables.iter_mut().find(|t| t.name == name) {
                            row.over.cursor = Some(CursorStart::Custom);
                            row.over.start_id = Some("0".into());
                            row.over.end_id = Some("20".into());
                        }
                        eprintln!("[dst-smoke] 已勾选 {name}（估算 {est} 行，区间 0..20），等待人工点开始");
                    }
                    this.invalidate_report();
                    cx.notify();
                });
                return;
            }

            // ②b 完整链路：确认建库 → 预检 → 自动开始（RB_GPUI_SMOKE_DST=2）
            if std::env::var("RB_GPUI_SMOKE_DST").ok().as_deref() == Some("2") {
                let ready = this
                    .update(cx, |this, cx| {
                        let target = this.missing_target_schema()?;
                        // 挑行数最少的一张可同步表，并把区间压到 20 行，避免写太多
                        let pick = this
                            .tables
                            .iter()
                            .filter(|t| t.unsupported.is_none())
                            .min_by_key(|t| t.rows_estimate)
                            .map(|t| (t.name.clone(), t.rows_estimate))?;
                        let (name, est) = pick;
                        this.toggle_table(&name, true);
                        if let Some(row) = this.tables.iter_mut().find(|t| t.name == name) {
                            row.over.cursor = Some(CursorStart::Custom);
                            row.over.start_id = Some("0".into());
                            row.over.end_id = Some("20".into());
                        }
                        eprintln!("[dst-smoke] 勾选表 {name}（估算 {est} 行，区间 0..20）");
                        this.invalidate_report();
                        eprintln!("[dst-smoke] 缺库={:?}，走「确认创建并开始」", target);
                        this.create_target_db_and_start(target, cx);
                        Some(name)
                    })
                    .ok()
                    .flatten();
                if ready.is_some() {
                    for _ in 0..20 {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1000))
                            .await;
                        let done = this
                            .update(cx, |this, _| {
                                let running = this.preflighting
                                    || this.tasks.iter().any(|t| t.is_active());
                                let task = this.tasks.first().map(|t| {
                                    format!("{:?} {}/{} 行 日志{}条", t.status, t.total_done(), t.total_planned(), t.logs.len())
                                });
                                let has_db = this
                                    .dst_schemas
                                    .iter()
                                    .any(|s| Some(&s.name) == this.dst_schema.as_ref());
                                (running, task, has_db)
                            })
                            .unwrap_or((false, None, false));
                        eprintln!(
                            "[dst-smoke]   进行中={} 任务={:?} 目标库已出现在列表={}",
                            done.0, done.1, done.2
                        );
                        if !done.0 {
                            break;
                        }
                    }
                    dump!("②b 建库+同步之后");
                }
            }

            // ③ 手动挑一个和目标库不同名的目标库：只告警，不阻断
            let other = this
                .update(cx, |this, _| {
                    let src = this.src_schema.clone().unwrap_or_default();
                    this.dst_schemas
                        .iter()
                        .map(|s| s.name.clone())
                        .find(|n| *n != src)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            if !other.is_empty() {
                eprintln!("[dst-smoke] 手动把目标库改成 {other}");
                let _ = this.update(cx, |this, cx| this.choose_schema(DbSide::Dst, &other, cx));
                dump!("③ 手动选不同名目标库");
            }

            // ④ 源库再切一次：目标库应重新跟随同名
            let again = this
                .update(cx, |this, _| this.src_schemas.first().map(|s| s.name.clone()))
                .ok()
                .flatten();
            if let Some(name) = again {
                let _ = this.update(cx, |this, cx| this.choose_schema(DbSide::Src, &name, cx));
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                dump!("④ 再切源库后重新对齐");
            }
            eprintln!("[dst-smoke] 结束");
        })
        .detach();
    }

    // ------------------------------------------------------------ 基础访问

    pub(crate) fn panel(&self, side: DbSide) -> &ConnPanel {
        match side {
            DbSide::Src => &self.src,
            DbSide::Dst => &self.dst,
        }
    }

    pub(crate) fn panel_mut(&mut self, side: DbSide) -> &mut ConnPanel {
        match side {
            DbSide::Src => &mut self.src,
            DbSide::Dst => &mut self.dst,
        }
    }

    pub(crate) fn schemas(&self, side: DbSide) -> &[SchemaInfo] {
        match side {
            DbSide::Src => &self.src_schemas,
            DbSide::Dst => &self.dst_schemas,
        }
    }

    pub(crate) fn schema(&self, side: DbSide) -> Option<String> {
        match side {
            DbSide::Src => self.src_schema.clone(),
            DbSide::Dst => self.dst_schema.clone(),
        }
    }

    pub(crate) fn selected_tables(&self) -> Vec<&TableRow> {
        self.tables.iter().filter(|t| t.selected).collect()
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.tables.iter().filter(|t| t.selected).count()
    }

    pub(crate) fn estimated_rows(&self) -> u64 {
        self.selected_tables().iter().map(|t| t.rows_estimate).sum()
    }

    pub(crate) fn can_preflight(&self) -> bool {
        self.src_schema.is_some()
            && self.dst_schema.is_some()
            && self.selected_count() > 0
            && !self.preflighting
    }

    pub(crate) fn can_start(&self) -> bool {
        if self.preflighting {
            return false;
        }
        if let Some(r) = &self.report {
            return !r.any_blocking_error() && r.tables.iter().any(|t| t.syncable);
        }
        // 目标库还没建：允许点「加入任务并开始」，点了会先问要不要建库，再自动预检 + 开始。
        self.missing_target_schema().is_some() && self.selected_count() > 0
    }

    /// 目标下拉里已经加载出来的库名。
    pub(crate) fn dst_schema_exists(&self) -> bool {
        match self.dst_schema.as_deref() {
            Some(name) => self.dst_schemas.iter().any(|s| s.name == name),
            None => false,
        }
    }

    /// 目标端缺少的库名（只有目标库列表已经拉到时才判断，避免误报）。
    pub(crate) fn missing_target_schema(&self) -> Option<String> {
        if !self.dst_schemas_loaded || self.src_schema.is_none() {
            return None;
        }
        if self.dst_schema_exists() {
            return None;
        }
        self.dst_schema
            .clone()
            .or_else(|| self.dst_schema_wanted.clone())
    }

    /// 源库与目标库名字不同（不阻断，但要提示）。
    pub(crate) fn schema_names_differ(&self) -> bool {
        match (&self.src_schema, &self.dst_schema) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }

    pub(crate) fn invalidate_report(&mut self) {
        self.report = None;
    }

    pub(crate) fn push_log(&mut self, level: &str, message: impl Into<String>) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        self.logs.push(LogEntry {
            time: time.into(),
            level: level.to_string().into(),
            message: message.into().into(),
        });
        if self.logs.len() > 1000 {
            let drop = self.logs.len() - 1000;
            self.logs.drain(0..drop);
        }
    }

    pub(crate) fn is_same_instance(&self, cx: &App) -> bool {
        let src = self.src.conn_config(cx);
        let dst = self.dst.conn_config(cx);
        rowbridge_core::preflight::same_instance(&src, &dst)
    }

    pub(crate) fn is_same_physical_db(&self, cx: &App) -> bool {
        self.is_same_instance(cx) && self.src_schema.is_some() && self.src_schema == self.dst_schema
    }

    pub(crate) fn is_same_instance_cfg(&self) -> bool {
        let s_id = self.src.saved_id.as_deref();
        let d_id = self.dst.saved_id.as_deref();
        if s_id.is_some() && s_id == d_id {
            return true;
        }
        false
    }

    pub(crate) fn will_truncate_any(&self) -> Vec<String> {
        self.tables
            .iter()
            .filter(|t| t.selected)
            .filter(|t| match t.over.truncate {
                Some(true) => true,
                Some(false) => false,
                None => self.opt.truncate_target,
            })
            .map(|t| t.name.clone())
            .collect()
    }

    pub(crate) fn filtered_tables(&self, cx: &App) -> Vec<(usize, &TableRow)> {
        let filter = self.table_filter.read(cx).value().trim().to_lowercase();
        self.tables
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                if filter.is_empty() {
                    true
                } else {
                    t.name.to_lowercase().contains(&filter)
                }
            })
            .collect()
    }

    pub(crate) fn select_filtered_tables(&mut self, on: bool, cx: &App) {
        let filter = self.table_filter.read(cx).value().trim().to_lowercase();
        for row in self.tables.iter_mut() {
            if row.unsupported.is_none() {
                if filter.is_empty() || row.name.to_lowercase().contains(&filter) {
                    row.selected = on;
                }
            }
        }
        self.invalidate_report();
    }

    // ------------------------------------------------------------ 选择

    pub(crate) fn toggle_table(&mut self, name: &str, on: bool) {
        if let Some(row) = self.tables.iter_mut().find(|t| t.name == name) {
            row.selected = on;
        }
        self.invalidate_report();
    }

    pub(crate) fn select_all_tables(&mut self, on: bool) {
        for row in self.tables.iter_mut() {
            if row.unsupported.is_none() {
                row.selected = on;
            }
        }
        self.invalidate_report();
    }

    pub(crate) fn set_table_cursor(&mut self, name: &str, cursor: Option<CursorStart>) {
        if let Some(row) = self.tables.iter_mut().find(|t| t.name == name) {
            row.over.cursor = cursor;
            self.invalidate_report();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_table_start_id(&mut self, name: &str, val: Option<String>) {
        if let Some(row) = self.tables.iter_mut().find(|t| t.name == name) {
            row.over.start_id = val.filter(|s| !s.trim().is_empty());
            self.invalidate_report();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_table_end_id(&mut self, name: &str, val: Option<String>) {
        if let Some(row) = self.tables.iter_mut().find(|t| t.name == name) {
            row.over.end_id = val.filter(|s| !s.trim().is_empty());
            self.invalidate_report();
        }
    }

    pub(crate) fn reset_all_overrides(&mut self) {
        for row in self.tables.iter_mut() {
            row.over = Default::default();
        }
        self.invalidate_report();
    }

    pub(crate) fn start_edit(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.tables.iter().find(|t| t.name == name) else {
            return;
        };
        self.editing = Some(name.to_string());
        self.ed_cursor = Some(row.over.cursor.unwrap_or(CursorStart::Full));
        self.ed_truncate = Tri::from_option(row.over.truncate);
        self.ed_create_table = Tri::from_option(row.over.create_table);
        self.ed_strip_foreign_keys = Tri::from_option(row.over.strip_foreign_keys);
        self.ed_strip_auto_increment = Tri::from_option(row.over.strip_auto_increment);
        self.ed_map_collation = Tri::from_option(row.over.map_collation);
        self.ed_continue_on_error = Tri::from_option(row.over.continue_on_error);
        let filter = row.over.filter.clone().unwrap_or_default();
        let start = row.over.start_id.clone().unwrap_or_default();
        let end = row.over.end_id.clone().unwrap_or_default();
        set_input(&self.ed_filter, filter, window, cx);
        set_input(&self.ed_start, start, window, cx);
        set_input(&self.ed_end, end, window, cx);

        if let Some(cols) = self.table_columns_cache.get(name) {
            self.ed_columns = cols.clone();
            self.loading_ed_columns = false;
        } else if let Some(schema) = self.src_schema.clone() {
            self.ed_columns.clear();
            self.loading_ed_columns = true;
            let cfg = self.src.conn_config(cx);
            let tbl = name.to_string();
            let tbl_key = tbl.clone();
            let rx = ops::spawn(ops::list_table_columns(cfg, schema, tbl));
            cx.spawn(async move |this, cx| {
                let result = rx.await.unwrap_or_else(|e| Err(format!("任务中断: {e}")));
                let _ = this.update(cx, |this, cx| {
                    if let Ok(cols) = result {
                        this.table_columns_cache
                            .insert(tbl_key.clone(), cols.clone());
                        if this.editing.as_deref() == Some(&tbl_key) {
                            this.ed_columns = cols;
                            this.loading_ed_columns = false;
                        }
                    } else if this.editing.as_deref() == Some(&tbl_key) {
                        this.loading_ed_columns = false;
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(crate) fn insert_filter_snippet(
        &mut self,
        snippet: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.ed_filter.read(cx).value().to_string();
        let trimmed = current.trim_end();
        let next = if trimmed.is_empty() {
            snippet.to_string()
        } else {
            let lower = trimmed.to_ascii_lowercase();
            if lower.ends_with("and")
                || lower.ends_with("or")
                || trimmed.ends_with('(')
                || trimmed.ends_with('=')
                || trimmed.ends_with('>')
                || trimmed.ends_with('<')
            {
                format!("{trimmed} {snippet}")
            } else {
                format!("{trimmed} AND {snippet}")
            }
        };
        set_input(&self.ed_filter, next, window, cx);
    }

    pub(crate) fn apply_edit(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.editing.clone() else {
            return;
        };
        let filter = self
            .opt
            .text_of(&self.ed_filter, cx)
            .filter(|s| !s.trim().is_empty());
        let start = self.opt.text_of(&self.ed_start, cx);
        let end = self.opt.text_of(&self.ed_end, cx);
        if let Some(row) = self.tables.iter_mut().find(|t| t.name == name) {
            row.over.dst_table = None;
            row.over.filter = filter;
            row.over.cursor = Some(self.ed_cursor.unwrap_or(CursorStart::Full));
            row.over.start_id = start;
            row.over.end_id = end;
            row.over.truncate = self.ed_truncate.to_option();
            row.over.create_table = self.ed_create_table.to_option();
            row.over.strip_foreign_keys = self.ed_strip_foreign_keys.to_option();
            row.over.strip_auto_increment = self.ed_strip_auto_increment.to_option();
            row.over.map_collation = self.ed_map_collation.to_option();
            row.over.continue_on_error = self.ed_continue_on_error.to_option();
        }
        self.editing = None;
        self.invalidate_report();
    }

    // ------------------------------------------------------------ 数据源

    pub(crate) fn pick_saved(
        &mut self,
        side: DbSide,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conn) = self.saved.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        self.panel_mut(side).apply_saved(&conn, window, cx);
        match side {
            // 换了目标数据源：目标库列表稍后会重新加载，先清掉旧的目标库选择
            DbSide::Dst => {
                self.dst_schemas.clear();
                self.dst_schemas_loaded = false;
                self.dst_schema = None;
                self.dst_schema_wanted = None;
            }
            // 换了源数据源：库列表与表清单作废，库名沿用该数据源上次用的（没有就等列表回来再定）
            DbSide::Src => {
                self.src_schemas.clear();
                self.tables.clear();
                self.src_schema = conn.last_schema.clone();
            }
        }
        self.invalidate_report();
        self.load_schemas(side, cx);
    }

    #[allow(dead_code)]
    pub(crate) fn save_current(&mut self, side: DbSide, cx: &Context<Self>) {
        let cfg = self.panel(side).conn_config(cx);
        let name: String = self.panel(side).display_name(cx).to_string();
        let id = self
            .panel(side)
            .saved_id
            .clone()
            .unwrap_or_else(|| format!("conn-{}", now_millis()));
        let conn = SavedConnection {
            id: id.clone(),
            name,
            host: cfg.host,
            port: cfg.port,
            username: cfg.username,
            password: cfg.password,
            use_tls: cfg.use_tls,
            last_schema: self.schema(side),
        };
        match connections::upsert(conn) {
            Ok(list) => {
                self.saved = list;
                let panel = self.panel_mut(side);
                panel.saved_id = Some(id);
                panel.note = Some((true, "已保存数据源".into()));
            }
            Err(e) => self.panel_mut(side).note = Some((false, e.into())),
        }
    }

    pub(crate) fn remove_saved(&mut self, id: &str) {
        if let Ok(list) = connections::remove(id) {
            self.saved = list;
        }
        for side in [DbSide::Src, DbSide::Dst] {
            if self.panel(side).saved_id.as_deref() == Some(id) {
                let panel = self.panel_mut(side);
                panel.saved_id = None;
                panel.label = None;
            }
        }
    }

    pub(crate) fn save_new_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .opt
            .text_of(&self.new_source_name, cx)
            .unwrap_or_else(|| "未命名数据源".into());
        let host = self
            .opt
            .text_of(&self.new_source_host, cx)
            .unwrap_or_else(|| "127.0.0.1".into());
        let port: u16 = self
            .opt
            .text_of(&self.new_source_port, cx)
            .and_then(|p| p.parse().ok())
            .unwrap_or(3306);
        let username = self
            .opt
            .text_of(&self.new_source_user, cx)
            .unwrap_or_else(|| "root".into());
        let password = self
            .opt
            .text_of(&self.new_source_password, cx)
            .unwrap_or_default();
        let conn = SavedConnection {
            id: format!("conn-{}", now_millis()),
            name,
            host,
            port,
            username,
            password,
            use_tls: self.new_source_tls,
            last_schema: None,
        };
        if let Ok(list) = connections::upsert(conn) {
            self.saved = list;
            self.adding_source = false;
            set_input(&self.new_source_name, "", window, cx);
            set_input(&self.new_source_password, "", window, cx);
            self.push_log("info", "已新建并保存数据源");
        }
    }

    // ------------------------------------------------------------ 数据库读取

    #[allow(dead_code)]
    pub(crate) fn test_conn(&mut self, side: DbSide, cx: &mut Context<Self>) {
        let cfg = self.panel(side).conn_config(cx);
        self.panel_mut(side).busy = true;
        self.panel_mut(side).note = None;
        let rx = ops::spawn(ops::test_connection(cfg));
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|e| Err(format!("任务中断: {e}")));
            let _ = this.update(cx, |this, cx| {
                this.panel_mut(side).busy = false;
                match result {
                    Ok(msg) => this.panel_mut(side).note = Some((true, msg.into())),
                    Err(e) => this.panel_mut(side).note = Some((false, e.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 拉库列表；`auto_pick` 为真时顺带做「目标库跟源库同名」的对齐。
    pub(crate) fn load_schemas(&mut self, side: DbSide, cx: &mut Context<Self>) {
        self.load_schemas_inner(side, true, cx);
    }

    /// 只刷新列表，不改变当前选择。
    pub(crate) fn refresh_schemas(&mut self, side: DbSide, cx: &mut Context<Self>) {
        self.load_schemas_inner(side, false, cx);
    }

    fn load_schemas_inner(&mut self, side: DbSide, auto_pick: bool, cx: &mut Context<Self>) {
        let cfg = self.panel(side).conn_config(cx);
        self.panel_mut(side).busy = true;
        let rx = ops::spawn(ops::list_schemas(cfg));
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|e| Err(format!("任务中断: {e}")));
            let _ = this.update(cx, |this, cx| {
                this.panel_mut(side).busy = false;
                match result {
                    Ok(list) => {
                        let count = list.len();
                        match side {
                            DbSide::Src => this.src_schemas = list,
                            DbSide::Dst => {
                                this.dst_schemas = list;
                                this.dst_schemas_loaded = true;
                            }
                        }
                        let _ = count;
                        this.panel_mut(side).note = None;
                        if auto_pick {
                            this.auto_pick_schema(side);
                        }
                        if side == DbSide::Src && this.src_schema.is_some() {
                            this.load_tables(cx);
                        }
                    }
                    Err(e) => this.panel_mut(side).note = Some((false, e.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn auto_pick_schema(&mut self, side: DbSide) {
        match side {
            DbSide::Src => {
                if self.src_schema.is_none() {
                    if let Some(first) = self.src_schemas.first() {
                        self.src_schema = Some(first.name.clone());
                    }
                }
                self.sync_default_dst();
            }
            DbSide::Dst => self.sync_default_dst(),
        }
    }

    /// 目标库跟随源库同名：目标端有同名库就直接选中，没有就留空并记住要建的库名。
    pub(crate) fn sync_default_dst(&mut self) {
        let Some(src) = self.src_schema.clone() else {
            return;
        };
        // 目标数据源还没选/库列表还没拉到，先不动
        if !self.dst_schemas_loaded {
            return;
        }

        let before = (self.dst_schema.clone(), self.dst_schema_wanted.clone());

        if self.is_same_instance_cfg() {
            // 同实例下「同名库」就是源库本身，自我同步没有意义，退而选一个别的库
            if self.dst_schema.as_deref() == Some(src.as_str()) {
                self.dst_schema = None;
            }
            if self.dst_schema.is_none() {
                self.dst_schema = self
                    .dst_schemas
                    .iter()
                    .find(|s| s.name != src)
                    .map(|s| s.name.clone());
            }
            self.dst_schema_wanted = self.dst_schema.clone();
        } else if self.dst_schemas.iter().any(|s| s.name == src) {
            self.dst_schema = Some(src.clone());
            self.dst_schema_wanted = Some(src);
        } else {
            // 目标端没有同名库：下拉留空，开始前会提示创建
            self.dst_schema = None;
            self.dst_schema_wanted = Some(src);
        }

        if before != (self.dst_schema.clone(), self.dst_schema_wanted.clone()) {
            self.invalidate_report();
        }
    }

    /// 目标库不存在时，用户确认后：建库开关打开 → 预检 →（通过则）自动开始。
    pub(crate) fn create_target_db_and_start(&mut self, name: String, cx: &mut Context<Self>) {
        self.dst_schema = Some(name.clone());
        self.dst_schema_wanted = Some(name.clone());
        self.opt.create_database = true;
        self.start_after_preflight = true;
        self.invalidate_report();
        self.push_log(
            "info",
            format!("目标库 {name} 不存在，将在同步时按源库字符集创建"),
        );
        self.do_preflight(cx);
    }

    pub(crate) fn choose_schema(&mut self, side: DbSide, name: &str, cx: &mut Context<Self>) {
        match side {
            DbSide::Src => {
                self.src_schema = Some(name.to_string());
                self.invalidate_report();
                self.load_tables(cx);
                self.sync_default_dst();
            }
            DbSide::Dst => {
                self.dst_schema = Some(name.to_string());
                self.dst_schema_wanted = Some(name.to_string());
                self.invalidate_report();
            }
        }
        if let Some(id) = self.panel(side).saved_id.clone() {
            let _ = connections::remember_schema(&id, name);
        }
        cx.notify();
    }

    pub(crate) fn load_tables(&mut self, cx: &mut Context<Self>) {
        self.table_columns_cache.clear();
        let Some(schema) = self.src_schema.clone() else {
            self.tables.clear();
            return;
        };
        let cfg = self.src.conn_config(cx);
        self.loading_tables = true;
        let rx = ops::spawn(ops::list_tables(cfg, schema.clone()));
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|e| Err(format!("任务中断: {e}")));
            let _ = this.update(cx, |this, cx| {
                this.loading_tables = false;
                match result {
                    Ok(list) => {
                        let count = list.len();
                        this.tables = list
                            .into_iter()
                            .map(|m| TableRow {
                                name: m.name,
                                rows_estimate: m.rows_estimate,
                                primary_key: m.primary_key,
                                source_max_pk: None,
                                unsupported: m.unsupported_reason,
                                selected: false,
                                over: TableOverride::default(),
                            })
                            .collect();
                        this.push_log("info", format!("{schema} 共 {count} 张表"));
                    }
                    Err(e) => {
                        this.tables.clear();
                        this.src.note = Some((false, e.clone().into()));
                        this.push_log("error", e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ 任务配置

    pub(crate) fn build_task_config(&self, cx: &Context<Self>) -> Option<TaskConfig> {
        let src_schema = self.src_schema.clone()?;
        let dst_schema = self.dst_schema.clone()?;
        let tables: Vec<TableTaskConfig> = self
            .tables
            .iter()
            .filter(|t| t.selected)
            .map(|t| TableTaskConfig {
                table: t.name.clone(),
                src_schema: src_schema.clone(),
                dst_schema: dst_schema.clone(),
                dst_table: None,
                columns: Vec::new(),
                cursor_start: Some(t.over.cursor.unwrap_or(CursorStart::Full)),
                start_id: t.over.start_id.clone(),
                end_id: t.over.end_id.clone(),
                truncate_target: t.over.truncate,
                create_table: t.over.create_table,
                strip_foreign_keys: t.over.strip_foreign_keys,
                strip_auto_increment: t.over.strip_auto_increment,
                map_collation: t.over.map_collation,
                continue_on_error: t.over.continue_on_error,
                filter: t.over.filter.clone(),
            })
            .collect();
        if tables.is_empty() {
            return None;
        }
        let batch_size = self.opt.usize_of(&self.opt.batch_size, cx, 1000).max(1);
        let options = TaskOptions {
            parallelism: self.opt.usize_of(&self.opt.parallelism, cx, 2).max(1),
            batch_size,
            max_batch_size: self
                .opt
                .usize_of(&self.opt.max_batch_size, cx, 10_000)
                .max(batch_size),
            adaptive_batch: self.opt.adaptive_batch,
            cursor_start: CursorStart::Full,
            write_mode: WriteMode::Upsert,
            end_id: None,
            start_id: None,
            create_table: self.opt.create_table,
            create_database: self.opt.create_database,
            strip_foreign_keys: self.opt.strip_foreign_keys,
            strip_auto_increment: self.opt.strip_auto_increment,
            map_collation: self.opt.map_collation,
            add_missing_columns: self.opt.add_missing_columns,
            disable_foreign_key_checks: self.opt.disable_foreign_key_checks,
            truncate_target: self.opt.truncate_target,
            relax_sql_mode: self.opt.relax_sql_mode,
            continue_on_error: self.opt.continue_on_error,
            max_retries: 3,
        };
        Some(TaskConfig {
            source: self.src.conn_config(cx),
            target: self.dst.conn_config(cx),
            tables,
            options,
        })
    }

    // ------------------------------------------------------------ 预检 / 结构 / 同步

    pub(crate) fn do_preflight(&mut self, cx: &mut Context<Self>) {
        let Some(cfg) = self.build_task_config(cx) else {
            self.status = Some((false, "请先选择库和表".into()));
            return;
        };
        self.preflighting = true;
        self.status = None;
        let rx = ops::spawn(ops::run_preflight(cfg));
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|e| Err(format!("任务中断: {e}")));
            let _ = this.update(cx, |this, cx| {
                this.preflighting = false;
                match result {
                    Ok(report) => {
                        for pf in &report.tables {
                            if let Some(tbl) = this.tables.iter_mut().find(|t| t.name == pf.table) {
                                tbl.source_max_pk = pf.source_max_pk.clone();
                            }
                        }
                        let blocked = report.tables.iter().filter(|t| !t.syncable).count();
                        this.push_log(
                            "info",
                            format!("预检完成：{} 张表，{} 张阻塞", report.tables.len(), blocked),
                        );
                        let passed = blocked == 0 && !report.any_blocking_error();
                        this.report = Some(report);
                        this.status = Some((
                            passed,
                            if passed {
                                "预检通过".into()
                            } else {
                                format!("{blocked} 张表被阻塞").into()
                            },
                        ));
                        // 目标库不存在 → 确认创建 → 预检通过后直接开始
                        if std::mem::take(&mut this.start_after_preflight) {
                            if this.can_start() {
                                this.do_start(cx);
                            } else {
                                this.push_log("warn", "建库后预检未通过，已取消自动开始");
                            }
                        }
                    }
                    Err(e) => {
                        this.report = None;
                        this.status = Some((false, format!("预检失败：{e}").into()));
                        this.push_log("error", format!("预检失败：{e}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn do_sync_schema(&mut self, cx: &mut Context<Self>) {
        let (Some(cfg), Some(report)) = (self.build_task_config(cx), self.report.clone()) else {
            return;
        };
        self.syncing_schema = true;
        let rx = ops::spawn(ops::run_sync_schema(cfg, report));
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or_else(|e| Err(format!("任务中断: {e}")));
            let _ = this.update(cx, |this, cx| {
                this.syncing_schema = false;
                match result {
                    Ok(r) => {
                        this.push_log(
                            "info",
                            format!(
                                "表结构同步：建库 {} / 建表 {} / 补列 {} / {}ms",
                                r.databases_created.len(),
                                r.tables_created.len(),
                                r.columns_added.len(),
                                r.elapsed_ms
                            ),
                        );
                        this.status = Some((true, "表结构同步完成".into()));
                    }
                    Err(e) => {
                        this.push_log("error", format!("表结构同步失败：{e}"));
                        this.status = Some((false, format!("表结构同步失败：{e}").into()));
                    }
                }
                cx.notify();
            });
            // 结构变了，重新预检
            let _ = this.update(cx, |this, cx| this.do_preflight(cx));
        })
        .detach();
    }

    pub(crate) fn do_start(&mut self, cx: &mut Context<Self>) {
        let (Some(cfg), Some(report)) = (self.build_task_config(cx), self.report.clone()) else {
            return;
        };

        let task_id = format!("task_{}", chrono::Local::now().format("%m%d_%H%M%S"));
        let cancel = CancellationToken::new();
        let pause = rowbridge_core::engine::PauseController::new();

        // 任务卡上要能区分「哪个数据源 → 哪个数据源」，所以把连接展示名与实例地址一并留存。
        let src_name = self.panel(DbSide::Src).display_name(cx).to_string();
        let dst_name = self.panel(DbSide::Dst).display_name(cx).to_string();
        let src_ep = format!("{}:{}", self.src.host_text(cx), self.src.port_text(cx));
        let dst_ep = format!("{}:{}", self.dst.host_text(cx), self.dst.port_text(cx));
        let src_schema = self.src_schema.clone().unwrap_or_default();
        let dst_schema = self.dst_schema.clone().unwrap_or_default();

        let table_names: Vec<String> = report.tables.iter().map(|t| t.table.clone()).collect();
        let task_name = format!("{src_schema} → {dst_schema}");

        let mut progress = HashMap::new();
        for t in &report.tables {
            progress.insert(t.table.clone(), TableProgress::pending(t.planned_rows));
        }

        let created_at = chrono::Local::now().format("%H:%M:%S").to_string();
        let initial_log = TaskLogEntry {
            time: created_at.clone().into(),
            level: "info".into(),
            table: None,
            message: format!("开始同步 {} 张表", table_names.len()).into(),
        };

        let store_src = format!("{src_ep}/{src_schema}");
        let store_dst = format!("{dst_ep}/{dst_schema}");
        let task = SyncTask {
            id: task_id.clone(),
            name: task_name,
            created_at,
            src_name,
            dst_name,
            src_ep,
            dst_ep,
            src_schema,
            dst_schema,
            tables: table_names,
            status: TaskStatus::Running,
            cancel: cancel.clone(),
            pause: pause.clone(),
            progress,
            logs: vec![initial_log],
            error: None,
        };

        self.tasks.insert(0, task);
        self.selected_task_id = Some(task_id.clone());
        store::record_task_start(&task_id, &store_src, &store_dst, "running", now_ms())
            .unwrap_or_else(|e| eprintln!("[store] 任务落库失败：{e}"));

        let handle = ops::start_sync(cfg, report.clone(), cancel, pause);
        self.status = Some((true, "已加入同步任务，可继续添加".into()));
        self.push_log(
            "info",
            format!("已创建同步任务 [{task_id}]，进度见「进行中」"),
        );

        // 任务已交给「进行中」，同步界面立即回到空白批次状态（保留源/目标库与连接），
        // 不跳转页签，用户可以直接继续勾表、预检、创建下一个任务。
        self.reset_batch();

        let tid = task_id.clone();
        cx.spawn(async move |this, cx| {
            let mut events = handle.events;
            let done = handle.done;
            let mut last_notify = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap_or_else(std::time::Instant::now);
            let mut persisted_logs: usize = 0;
            let mut last_persist = std::time::Instant::now();
            while let Some(ev) = events.recv().await {
                let tid = tid.clone();
                let is_batch = matches!(ev, Event::BatchDone { .. });
                let should_notify =
                    !is_batch || last_notify.elapsed() >= std::time::Duration::from_millis(60);
                if should_notify {
                    last_notify = std::time::Instant::now();
                }
                let _ = this.update(cx, |this, cx| {
                    if let Some(task) = this.tasks.iter_mut().find(|t| t.id == tid) {
                        Self::apply_task_event(task, &ev);
                        Self::persist_task_event(
                            &tid,
                            &task.logs,
                            &mut persisted_logs,
                            &mut last_persist,
                        );
                    }
                    this.on_event(ev);
                    if should_notify {
                        cx.notify();
                    }
                });
            }
            let result = done.await;
            let tid = tid.clone();
            let _ = this.update(cx, |this, cx| {
                if let Some(task) = this.tasks.iter_mut().find(|t| t.id == tid) {
                    let finish = |task: &SyncTask, error: Option<&str>, elapsed_ms: u64| {
                        let total: u64 = task.progress.values().map(|p| p.rows).sum();
                        let done: usize = task
                            .progress
                            .values()
                            .filter(|p| p.status == "done")
                            .count();
                        let failed_t: usize = task
                            .progress
                            .values()
                            .filter(|p| p.status == "failed")
                            .count();
                        store::record_task_finish(
                            &task.id,
                            task.status.store_key(),
                            total,
                            done,
                            failed_t,
                            elapsed_ms,
                            now_ms(),
                            error,
                        )
                        .unwrap_or_else(|e| eprintln!("[store] 任务状态落库失败：{e}"));
                    };
                    match &result {
                        Ok(Ok(r)) => {
                            task.status = if r.cancelled {
                                TaskStatus::Cancelled
                            } else if r.failed > 0 {
                                TaskStatus::Failed
                            } else {
                                TaskStatus::Completed
                            };
                            let msg = format!(
                                "同步完成：共 {} 行 · 成功 {} · 失败 {} · 部分 {} · {}ms",
                                r.total_rows, r.succeeded, r.failed, r.partial, r.elapsed_ms
                            );
                            task.logs.push(TaskLogEntry {
                                time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                                level: if r.failed == 0 {
                                    "info".into()
                                } else {
                                    "warn".into()
                                },
                                table: None,
                                message: msg.into(),
                            });
                            finish(task, None, r.elapsed_ms);
                        }
                        Ok(Err(e)) => {
                            task.status = TaskStatus::Failed;
                            task.error = Some(e.clone());
                            task.logs.push(TaskLogEntry {
                                time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                                level: "error".into(),
                                table: None,
                                message: format!("任务失败：{e}").into(),
                            });
                            finish(task, Some(e), 0);
                        }
                        Err(e) => {
                            task.status = TaskStatus::Failed;
                            task.error = Some(e.to_string());
                            finish(task, Some(&task.error.clone().unwrap_or_default()), 0);
                        }
                    }
                }
                match result {
                    Ok(Ok(r)) => {
                        let msg = format!(
                            "完成：{} 行 · 成功 {} · 失败 {} · 部分 {} · {}ms",
                            r.total_rows, r.succeeded, r.failed, r.partial, r.elapsed_ms
                        );
                        this.push_log("info", msg.clone());
                        this.status = Some((r.failed == 0 && !r.cancelled, msg.into()));
                    }
                    Ok(Err(e)) => {
                        this.push_log("error", format!("任务失败：{e}"));
                        this.status = Some((false, format!("任务失败：{e}").into()));
                    }
                    Err(e) => {
                        this.status = Some((false, format!("任务通道中断：{e}").into()));
                    }
                }
                // 同步过程中可能建了新库，刷新目标库列表，但不改动当前选择
                this.refresh_schemas(DbSide::Dst, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_task_event(task: &mut SyncTask, ev: &Event) {
        match ev {
            Event::TableStarted { table, .. } => {
                if let Some(p) = task.progress.get_mut(table) {
                    p.status = "running";
                }
            }
            Event::BatchDone {
                table,
                rows,
                cursor,
                rows_per_sec,
                ..
            } => {
                if let Some(p) = task.progress.get_mut(table) {
                    p.rows += rows;
                    p.cursor = cursor.clone();
                    p.rps = *rows_per_sec;
                    p.status = "running";
                }
            }
            Event::TableDone { table, rows, .. } => {
                if let Some(p) = task.progress.get_mut(table) {
                    p.rows = *rows;
                    p.status = "done";
                    p.rps = 0.0;
                }
                task.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "info".into(),
                    table: Some(table.clone()),
                    message: format!("表 {table} 同步完成，共 {rows} 行").into(),
                });
            }
            Event::TableFailed { table, error } => {
                if let Some(p) = task.progress.get_mut(table) {
                    p.status = "failed";
                    p.error = Some(error.clone());
                }
                task.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "error".into(),
                    table: Some(table.clone()),
                    message: format!("表 {table} 失败：{error}").into(),
                });
            }
            Event::Log {
                table,
                level,
                message,
            } => {
                task.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: level.clone().into(),
                    table: table.clone(),
                    message: message.clone().into(),
                });
            }
            Event::Warning { table, message } => {
                task.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "warn".into(),
                    table: table.clone(),
                    message: message.clone().into(),
                });
            }
            Event::Cancelled => {
                task.status = TaskStatus::Cancelled;
                task.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "warn".into(),
                    table: None,
                    message: "任务已取消".into(),
                });
            }
            _ => {}
        }
    }

    /// 把任务里「还没落库」的日志增量写进 SQLite。
    /// 增量由 `last_persist` 记录：GPUI 主线程串行调用，闭包拿到的 idx 是快照。
    fn persist_task_event(
        tid: &str,
        logs: &[TaskLogEntry],
        persisted: &mut usize,
        last_persist: &mut std::time::Instant,
    ) {
        // 日志太频繁：前两条（任务开始类）不落；之后按节流批量追加，
        // 避免拖慢高频 BatchDone 的 UI 刷新。
        let start = (*persisted).max(2).min(logs.len());
        if start >= logs.len() {
            return;
        }
        if last_persist.elapsed() < std::time::Duration::from_millis(1500) && logs.len() <= 500 {
            return;
        }
        for entry in &logs[start..] {
            let _ = store::append_log(
                tid,
                now_ms(),
                entry.level.as_ref(),
                entry.table.as_deref(),
                entry.message.as_ref(),
            );
        }
        *persisted = logs.len();
        *last_persist = std::time::Instant::now();
    }

    pub(crate) fn pause_task(&mut self, id: &str) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            if t.status == TaskStatus::Running {
                t.pause.pause();
                t.status = TaskStatus::Paused;
                t.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "warn".into(),
                    table: None,
                    message: "任务已暂停".into(),
                });
            }
        }
    }

    pub(crate) fn resume_task(&mut self, id: &str) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            if t.status == TaskStatus::Paused {
                t.pause.resume();
                t.status = TaskStatus::Running;
                t.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "info".into(),
                    table: None,
                    message: "任务恢复运行".into(),
                });
            }
        }
    }

    pub(crate) fn cancel_task(&mut self, id: &str) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            if t.is_active() {
                t.cancel.cancel();
                t.pause.resume();
                t.status = TaskStatus::Cancelled;
                t.logs.push(TaskLogEntry {
                    time: chrono::Local::now().format("%H:%M:%S").to_string().into(),
                    level: "warn".into(),
                    table: None,
                    message: "已发送取消请求".into(),
                });
            }
        }
    }

    pub(crate) fn remove_task(&mut self, id: &str) {
        if let Some(pos) = self.tasks.iter().position(|t| t.id == id) {
            let t = &self.tasks[pos];
            if t.is_active() {
                t.cancel.cancel();
                t.pause.resume();
            }
            self.tasks.remove(pos);
            store::remove_task(id).unwrap_or_else(|e| eprintln!("[store] 删除任务记录失败：{e}"));
            if self.selected_task_id.as_deref() == Some(id) {
                self.selected_task_id = self.tasks.first().map(|t| t.id.clone());
            }
        }
    }

    /// 选中历史任务时按需加载它的日志（内存里没有就跑一次 SQLite）。
    pub(crate) fn load_task_logs(&mut self, id: &str) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if !task.logs.is_empty() || task.is_active() {
            return;
        }
        match store::task_logs(id, 500) {
            Ok(rows) => {
                task.logs = rows
                    .into_iter()
                    .map(|r| TaskLogEntry {
                        time: chrono::DateTime::from_timestamp(r.ts / 1000, 0)
                            .map(|t| t.format("%H:%M:%S").to_string())
                            .unwrap_or_default()
                            .into(),
                        level: r.level.into(),
                        table: r.table_name,
                        message: r.message.into(),
                    })
                    .collect();
            }
            Err(e) => {
                eprintln!("[store] 加载任务日志失败：{e}");
            }
        }
    }

    /// 启动时从 SQLite 恢复历史任务（只做展示，不带运行时控制）。
    pub(crate) fn load_history_tasks() -> Vec<SyncTask> {
        match store::recent_tasks(100) {
            Ok(rows) => rows
                .into_iter()
                .map(|r| {
                    SyncTask::from_history(
                        r.id,
                        r.src,
                        r.dst,
                        &r.status,
                        r.total_rows,
                        r.started_at,
                        r.error,
                    )
                })
                .collect(),
            Err(e) => {
                eprintln!("[store] 加载历史任务失败：{e}");
                Vec::new()
            }
        }
    }

    pub(crate) fn reset_batch(&mut self) {
        for row in self.tables.iter_mut() {
            row.selected = false;
            row.over = Default::default();
        }
        self.report = None;
        self.editing = None;
        self.show_advanced = false;
    }

    fn on_event(&mut self, ev: Event) {
        use rowbridge_core::engine::Event as E;
        match ev {
            E::TaskStarted { tables } => {
                self.push_log("info", format!("引擎启动：{tables} 张表"));
            }
            E::TableStarted {
                table,
                columns,
                needs_create_table,
            } => {
                if needs_create_table {
                    self.push_log("info", format!("{table}：目标表将被创建"));
                }
                self.push_log("info", format!("{table}：开始（{columns} 列）"));
            }
            E::TableCreated { table, .. } => {
                self.push_log("info", format!("{table}：已自动建表"));
            }
            E::BatchDone {
                table,
                rows,
                total_rows,
                cursor,
                rows_per_sec,
                batch_size,
                elapsed_ms,
            } => {
                let _ = (
                    table,
                    rows,
                    total_rows,
                    cursor,
                    rows_per_sec,
                    batch_size,
                    elapsed_ms,
                );
            }
            E::TableDone {
                table,
                rows,
                elapsed_ms,
            } => {
                self.push_log("info", format!("{table}：完成 {rows} 行 / {elapsed_ms}ms"));
            }
            E::TableFailed { table, error } => {
                self.push_log("error", format!("{table}：失败 {error}"));
            }
            E::Warning { table, message } => {
                let prefix = table.unwrap_or_default();
                self.push_log("warn", format!("{prefix} {message}"));
            }
            E::Log {
                table,
                level,
                message,
            } => {
                let prefix = table.unwrap_or_default();
                self.push_log(&level, format!("{prefix} {message}"));
            }
            E::TaskDone {
                total_rows,
                succeeded,
                failed,
                elapsed_ms,
            } => {
                self.push_log(
                    "info",
                    format!("任务结束：{total_rows} 行 / 成功 {succeeded} / 失败 {failed} / {elapsed_ms}ms"),
                );
            }
            E::Cancelled => {
                self.push_log("warn", "任务已取消");
            }
        }
    }
}

pub(crate) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 当前毫秒时间戳（SQLite 记录用）。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
