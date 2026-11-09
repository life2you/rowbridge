//! RowBridge 主视图状态与动作（GPUI 版）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use rowbridge_core::config::{CursorStart, TableTaskConfig, TaskConfig, TaskOptions, WriteMode};
use rowbridge_core::connections::{self, SavedConnection};
use rowbridge_core::engine::Event;
use rowbridge_core::introspect::SchemaInfo;
use rowbridge_core::preflight::PreflightReport;
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
    pub(crate) src_schema: Option<String>,
    pub(crate) dst_schema: Option<String>,
    pub(crate) tables: Vec<TableRow>,
    pub(crate) loading_tables: bool,

    /// 正在编辑覆盖参数的表名
    pub(crate) editing: Option<String>,
    pub(crate) ed_cursor: Option<CursorStart>,
    pub(crate) ed_truncate: Tri,
    pub(crate) ed_dst_table: Entity<InputState>,
    pub(crate) ed_start: Entity<InputState>,
    pub(crate) ed_end: Entity<InputState>,

    pub(crate) report: Option<PreflightReport>,
    pub(crate) preflighting: bool,
    pub(crate) syncing_schema: bool,
    pub(crate) running: bool,
    pub(crate) progress: HashMap<String, TableProgress>,
    pub(crate) logs: Vec<(SharedString, SharedString)>,
    pub(crate) status: Option<(bool, SharedString)>,
    pub(crate) cancel: Option<CancellationToken>,

    pub(crate) view: Tab,
    pub(crate) show_advanced: bool,
    pub(crate) smoke_done: bool,
    pub(crate) _subs: Vec<Subscription>,
}

impl RowBridgeApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = connections::init();
        let saved = connections::list().unwrap_or_default();

        let src = ConnPanel::new("用户名", window, cx);
        let dst = ConnPanel::new("用户名", window, cx);
        let opt = OptionsPanel::new(window, cx);
        let ed_dst_table = cx.new(|cx| {
            InputState::new(window, cx).placeholder("目标表名，留空表示同名")
        });
        let ed_start = cx.new(|cx| InputState::new(window, cx).placeholder("起始 ID（含）"));
        let ed_end = cx.new(|cx| InputState::new(window, cx).placeholder("结束 ID（含）"));

        let mut subs = Vec::new();
        let watched: Vec<Entity<InputState>> = src
            .inputs()
            .into_iter()
            .chain(dst.inputs())
            .chain(opt.inputs())
            .chain([ed_dst_table.clone(), ed_start.clone(), ed_end.clone()])
            .collect();
        for state in watched {
            subs.push(cx.subscribe_in(&state, window, |_, _, _: &InputEvent, _, cx| {
                cx.notify();
            }));
        }

        Self {
            src,
            dst,
            opt,
            saved,
            src_schemas: Vec::new(),
            dst_schemas: Vec::new(),
            src_schema: None,
            dst_schema: None,
            tables: Vec::new(),
            loading_tables: false,
            editing: None,
            ed_cursor: None,
            ed_truncate: Tri::Follow,
            ed_dst_table,
            ed_start,
            ed_end,
            report: None,
            preflighting: false,
            syncing_schema: false,
            running: false,
            progress: HashMap::new(),
            logs: Vec::new(),
            status: None,
            cancel: None,
            view: Tab::Sync,
            show_advanced: false,
            smoke_done: false,
            _subs: subs,
        }
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
            && !self.running
    }

    pub(crate) fn can_start(&self) -> bool {
        match &self.report {
            Some(r) => {
                r.tables.iter().any(|t| t.syncable) && !self.running && !self.preflighting
            }
            None => false,
        }
    }

    pub(crate) fn invalidate_report(&mut self) {
        self.report = None;
        self.progress.clear();
    }

    pub(crate) fn push_log(&mut self, level: &str, message: impl Into<String>) {
        self.logs.push((level.to_string().into(), message.into().into()));
        if self.logs.len() > 600 {
            let drop = self.logs.len() - 600;
            self.logs.drain(0..drop);
        }
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

    pub(crate) fn start_edit(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.tables.iter().find(|t| t.name == name) else {
            return;
        };
        self.editing = Some(name.to_string());
        self.ed_cursor = row.over.cursor;
        self.ed_truncate = Tri::from_option(row.over.truncate);
        let dst_table = row.over.dst_table.clone().unwrap_or_default();
        let start = row.over.start_id.clone().unwrap_or_default();
        let end = row.over.end_id.clone().unwrap_or_default();
        set_input(&self.ed_dst_table, dst_table, window, cx);
        set_input(&self.ed_start, start, window, cx);
        set_input(&self.ed_end, end, window, cx);
    }

    pub(crate) fn apply_edit(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.editing.clone() else {
            return;
        };
        let dst_table = self
            .opt
            .text_of(&self.ed_dst_table, cx)
            .filter(|s| !s.is_empty());
        let start = self.opt.text_of(&self.ed_start, cx);
        let end = self.opt.text_of(&self.ed_end, cx);
        if let Some(row) = self.tables.iter_mut().find(|t| t.name == name) {
            row.over.dst_table = dst_table;
            row.over.cursor = self.ed_cursor;
            row.over.start_id = start;
            row.over.end_id = end;
            row.over.truncate = self.ed_truncate.to_option();
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
        if let Some(schema) = conn.last_schema.clone() {
            match side {
                DbSide::Src => self.src_schema = Some(schema),
                DbSide::Dst => self.dst_schema = Some(schema),
            }
        }
        self.invalidate_report();
        self.load_schemas(side, cx);
        if side == DbSide::Src {
            self.load_tables(cx);
        }
    }

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

    // ------------------------------------------------------------ 数据库读取

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

    pub(crate) fn load_schemas(&mut self, side: DbSide, cx: &mut Context<Self>) {
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
                            DbSide::Dst => this.dst_schemas = list,
                        }
                        this.panel_mut(side).note =
                            Some((true, format!("{count} 个库").into()));
                        this.auto_pick_schema(side);
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
                if self.src_schema.is_some() {
                    return;
                }
                if let Some(first) = self.src_schemas.first() {
                    self.src_schema = Some(first.name.clone());
                }
                self.sync_default_dst();
            }
            DbSide::Dst => {
                if self.dst_schema.is_some() {
                    return;
                }
                if self.auto_dst_schema().is_some() {
                    self.dst_schema = self.auto_dst_schema();
                } else if let Some(first) = self.dst_schemas.first() {
                    self.dst_schema = Some(first.name.clone());
                }
            }
        }
    }

    fn auto_dst_schema(&self) -> Option<String> {
        let src = self.src_schema.as_deref()?;
        self.dst_schemas
            .iter()
            .find(|s| s.name == src)
            .map(|s| s.name.clone())
    }

    pub(crate) fn sync_default_dst(&mut self) {
        if self.dst_schema.is_none() {
            if let Some(s) = self.auto_dst_schema() {
                self.dst_schema = Some(s);
            }
        }
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
                self.invalidate_report();
            }
        }
        if let Some(id) = self.panel(side).saved_id.clone() {
            let _ = connections::remember_schema(&id, name);
        }
    }

    pub(crate) fn load_tables(&mut self, cx: &mut Context<Self>) {
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
                dst_table: t.over.dst_table.clone().filter(|s| !s.trim().is_empty()),
                columns: Vec::new(),
                cursor_start: t.over.cursor,
                start_id: t.over.start_id.clone(),
                end_id: t.over.end_id.clone(),
                truncate_target: t.over.truncate,
            })
            .collect();
        if tables.is_empty() {
            return None;
        }
        let batch_size = self
            .opt
            .usize_of(&self.opt.batch_size, cx, 1000)
            .max(1);
        let options = TaskOptions {
            parallelism: self.opt.usize_of(&self.opt.parallelism, cx, 2).max(1),
            batch_size,
            max_batch_size: self
                .opt
                .usize_of(&self.opt.max_batch_size, cx, 10_000)
                .max(batch_size),
            adaptive_batch: self.opt.adaptive_batch,
            cursor_start: self.opt.cursor.to_core(),
            write_mode: WriteMode::Upsert,
            end_id: self.opt.text_of(&self.opt.global_end, cx),
            start_id: self.opt.text_of(&self.opt.global_start, cx),
            create_table: self.opt.create_table,
            create_database: self.opt.create_database,
            strip_foreign_keys: self.opt.strip_foreign_keys,
            strip_auto_increment: self.opt.strip_auto_increment,
            map_collation: self.opt.map_collation,
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
                        this.seed_progress(&report);
                        let blocked = report.tables.iter().filter(|t| !t.syncable).count();
                        this.push_log(
                            "info",
                            format!(
                                "预检完成：{} 张表，{} 张阻塞",
                                report.tables.len(),
                                blocked
                            ),
                        );
                        this.report = Some(report);
                        this.status = Some((
                            blocked == 0,
                            if blocked == 0 {
                                "预检通过".into()
                            } else {
                                format!("{blocked} 张表被阻塞").into()
                            },
                        ));
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

    fn seed_progress(&mut self, report: &PreflightReport) {
        self.progress.clear();
        for t in &report.tables {
            let planned = t.planned_rows;
            self.progress
                .insert(t.table.clone(), TableProgress::pending(planned));
        }
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
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(name) = this.tables.iter().find(|t| t.selected).map(|t| t.name.clone()) {
                    this.start_edit(&name, window, cx);
                    this.ed_cursor = Some(CursorStart::Custom);
                    set_input(&this.ed_start, "1", window, cx);
                    set_input(&this.ed_end, "500", window, cx);
                    this.apply_edit(cx);
                    println!("[smoke] 表级覆盖已写入: {:?}", this.tables.iter().find(|t| t.name == name).map(|t| t.over.clone()));
                }
            });
            let _ = this.update(cx, |this, cx| this.do_preflight(cx));
        })
        .detach();
    }

    pub(crate) fn do_start(&mut self, cx: &mut Context<Self>) {
        let (Some(cfg), Some(report)) = (self.build_task_config(cx), self.report.clone()) else {
            return;
        };
        let handle = ops::start_sync(cfg, report.clone());
        self.cancel = Some(handle.cancel.clone());
        self.running = true;
        self.status = Some((true, "同步中…".into()));
        self.push_log("info", format!("开始同步 {} 张表", report.tables.len()));

        cx.spawn(async move |this, cx| {
            let mut events = handle.events;
            let done = handle.done;
            while let Some(ev) = events.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.on_event(ev);
                    cx.notify();
                });
            }
            let result = done.await;
            let _ = this.update(cx, |this, cx| {
                this.running = false;
                this.cancel = None;
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
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn do_cancel(&mut self) {
        if let Some(token) = &self.cancel {
            token.cancel();
            self.status = Some((true, "正在取消…".into()));
        }
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
                let entry = self
                    .progress
                    .entry(table.clone())
                    .or_insert_with(|| TableProgress::pending(0));
                entry.status = "running";
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
                let planned = self
                    .report
                    .as_ref()
                    .and_then(|r| r.tables.iter().find(|t| t.table == table))
                    .map(|t| t.planned_rows)
                    .unwrap_or(0);
                let entry = self
                    .progress
                    .entry(table.clone())
                    .or_insert_with(|| TableProgress::pending(planned));
                entry.status = "running";
                entry.rows = total_rows;
                entry.rps = rows_per_sec;
                entry.cursor = cursor;
                let _ = (rows, batch_size, elapsed_ms);
            }
            E::TableDone {
                table,
                rows,
                elapsed_ms,
            } => {
                if let Some(entry) = self.progress.get_mut(&table) {
                    entry.status = "done";
                    entry.rows = rows;
                }
                self.push_log(
                    "info",
                    format!("{table}：完成 {rows} 行 / {elapsed_ms}ms"),
                );
            }
            E::TableFailed { table, error } => {
                if let Some(entry) = self.progress.get_mut(&table) {
                    entry.status = "failed";
                    entry.error = Some(error.clone());
                }
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
