//! 三栏面板：连接编辑、库列表、表列表、批次选项与进度。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    checkbox::Checkbox, input::Input, progress::Progress, scroll::ScrollableElement as _, *,
};
use gpui_kit::*;
use rowbridge_core::config::CursorStart;
use rowbridge_core::preflight::Severity;

use crate::app::RowBridgeApp;
use crate::model::*;
use crate::views::{badge, label, section_title};

impl RowBridgeApp {
    pub(crate) fn side_pane(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let mut pane = v_flex()
            .w(px(300.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .border_color(border);
        pane = match side {
            DbSide::Src => pane.border_r_1(),
            DbSide::Dst => pane.border_l_1(),
        };
        let mut pane = pane
            .child(self.conn_editor(side, cx))
            .child(self.schema_list(side, cx));
        pane = match side {
            DbSide::Src => pane.child(self.table_list(cx)),
            DbSide::Dst => pane.child(self.dst_stats(cx)),
        };
        pane.into_any_element()
    }

    // ------------------------------------------------------------ 连接编辑

    fn conn_editor(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let panel = self.panel(side);
        let tag = match side {
            DbSide::Src => "src",
            DbSide::Dst => "dst",
        };

        let mut note = h_flex().min_h(px(16.));
        match &panel.note {
            Some((ok, msg)) => {
                note = note.child(label(
                    msg.clone(),
                    if *ok { success } else { danger },
                ));
            }
            None => {
                note = note.child(label("", muted));
            }
        }

        let mut block = v_flex()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(section_title(side.title(), fg))
                    .child(label(
                        if panel.busy { "连接中…" } else { "" },
                        muted,
                    )),
            )
            .child(self.saved_chips(side, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&panel.host).small()))
                    .child(div().w(px(84.)).child(Input::new(&panel.port).small())),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&panel.user).small()))
                    .child(div().flex_1().min_w_0().child(Input::new(&panel.password).small())),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Checkbox::new(format!("tls-{tag}"))
                            .small()
                            .checked(panel.use_tls)
                            .label("TLS")
                            .on_change(cx.listener(move |this, checked, _, cx| {
                                this.panel_mut(side).use_tls = *checked;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(format!("test-{tag}"))
                            .xsmall()
                            .label("测试")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.test_conn(side, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(format!("load-{tag}"))
                            .xsmall()
                            .label("加载库")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.load_schemas(side, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(format!("save-{tag}"))
                            .xsmall()
                            .ghost()
                            .label("保存")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.save_current(side, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(note);

        if let Some(name) = &panel.label {
            block = block.child(label(format!("数据源：{name}"), muted));
        }

        block.into_any_element()
    }

    fn saved_chips(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let tag = match side {
            DbSide::Src => "src",
            DbSide::Dst => "dst",
        };
        let mut row = h_flex().gap_1().flex_wrap();
        if self.saved.is_empty() {
            row = row.child(label("暂无已保存数据源", muted));
        } else {
            for conn in self.saved.iter().take(6) {
                let id = conn.id.clone();
                let text = conn.name.clone();
                row = row.child(
                    Button::new(format!("chip-{tag}-{id}"))
                        .xsmall()
                        .ghost()
                        .label(text)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.pick_saved(side, &id, window, cx);
                            cx.notify();
                        })),
                );
            }
        }
        row.into_any_element()
    }

    // ------------------------------------------------------------ 库列表

    fn schema_list(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let primary = cx.theme().primary;
        let primary_fg = cx.theme().primary_foreground;
        let hover_bg = cx.theme().muted;
        let current = self.schema(side);
        let schemas = self.schemas(side).to_vec();

        let mut rows = v_flex().gap_1();
        if schemas.is_empty() {
            rows = rows.child(label("点「加载库」读取数据库列表", muted));
        }
        for s in schemas {
            let name = s.name.clone();
            let selected = current.as_deref() == Some(name.as_str());
            let mut row = h_flex()
                .id(format!("sc-{}-{name}", side as u8))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .child(name.clone());
            row = if selected {
                row.bg(primary).text_color(primary_fg)
            } else {
                row.hover(move |s| s.bg(hover_bg))
            };
            rows = rows.child(row.on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.choose_schema(side, &name, cx);
                    cx.notify();
                }),
            ));
        }

        v_flex()
            .flex_shrink_0()
            .gap_1()
            .px_3()
            .pb_2()
            .child(section_title("数据库", fg))
            .child(
                div()
                    .id(format!("schema-{:?}", side as u8))
                    .max_h(px(180.))
                    .overflow_y_scrollbar()
                    .child(rows),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ 表列表

    fn table_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let selected_bg = cx.theme().muted;
        let border = cx.theme().border;

        let mut header = h_flex()
            .items_center()
            .justify_between()
            .px_3()
            .pt_2()
            .pb_1()
            .child(section_title(
                format!("表 · 已选 {}", self.selected_count()),
                fg,
            ));
        header = header
            .child(
                Button::new("sel-all")
                    .xsmall()
                    .ghost()
                    .label("全选")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.select_all_tables(true);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("sel-none")
                    .xsmall()
                    .ghost()
                    .label("清空")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.select_all_tables(false);
                        cx.notify();
                    })),
            );

        let mut list = v_flex().gap_1().px_2().pb_3();
        if self.loading_tables {
            list = list.child(label("加载表清单…", muted));
        } else if self.tables.is_empty() {
            list = list.child(label("选择左侧数据库后自动加载", muted));
        }
        for (i, row) in self.tables.iter().enumerate() {
            let name = row.name.clone();
            let toggle_name = row.name.clone();
            let selected = row.selected;
            let disabled = row.unsupported.is_some();
            let mut item = h_flex()
                .gap_2()
                .items_center()
                .px_1()
                .py_1()
                .rounded_md()
                .child(
                    Checkbox::new(format!("tbl-{i}"))
                        .small()
                        .checked(selected)
                        .disabled(disabled)
                        .on_change(cx.listener(move |this, checked, _, cx| {
                            this.toggle_table(&toggle_name, *checked);
                            cx.notify();
                        })),
                )
                .child(div().flex_1().min_w_0().text_xs().child(name.clone()))
                .child(label(human_rows(row.rows_estimate), muted));

            if let Some(reason) = &row.unsupported {
                item = item.child(badge(reason.clone(), danger, selected_bg));
            } else if let Some(pk) = &row.primary_key {
                item = item.child(label(format!("PK {pk}"), muted));
            }
            if selected {
                item = item.bg(selected_bg);
            }
            list = list.child(item);
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .border_t_1()
            .border_color(border)
            .child(header)
            .child(
                div()
                    .id("table-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(list),
            )
            .into_any_element()
    }

    fn dst_stats(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let success = cx.theme().success;
        let warning = cx.theme().warning;

        let create = self
            .report
            .as_ref()
            .map(|r| r.tables.iter().filter(|t| t.needs_create_table).count())
            .unwrap_or(0);
        let existing = self
            .report
            .as_ref()
            .map(|r| r.tables.iter().filter(|t| t.target_rows_exists).count())
            .unwrap_or(0);
        let alter: usize = self
            .report
            .as_ref()
            .map(|r| r.tables.iter().map(|t| t.alter_sqls.len()).sum())
            .unwrap_or(0);
        let has_schema_work = create > 0 || alter > 0;

        let mut block = v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .p_3()
            .border_t_1()
            .border_color(border)
            .child(section_title("目标状态", fg))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(label("将新建表", muted))
                            .child(label(create.to_string(), warning)),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .child(label("目标已有数据", muted))
                            .child(label(existing.to_string(), success)),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .child(label("待补列", muted))
                            .child(label(alter.to_string(), warning)),
                    ),
            )
            .child(
                Button::new("sync-schema")
                    .small()
                    .label(if self.syncing_schema {
                        "同步结构中…"
                    } else {
                        "同步表结构"
                    })
                    .disabled(!has_schema_work || self.syncing_schema || self.running)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.do_sync_schema(cx);
                        cx.notify();
                    })),
            )
            .child(label(
                "只建库 / 建表 / 补列，不搬运数据。",
                muted,
            ));

        block = block.child(self.report_block(cx));
        block.into_any_element()
    }

    // ------------------------------------------------------------ 中间批次区

    pub(crate) fn center_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .flex_1()
            .min_w(px(420.))
            .min_h_0()
            .child(self.batch_options(cx))
            .child(self.selected_list(cx))
            .child(self.editor_panel(cx))
            .child(self.progress_block(cx))
            .into_any_element()
    }

    fn batch_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let primary = cx.theme().primary;
        let primary_fg = cx.theme().primary_foreground;

        let mut cursor_row = h_flex().gap_1().items_center();
        for mode in [CursorMode::Full, CursorMode::Resume, CursorMode::Custom] {
            let button = Button::new(format!("gm-{}", mode.label()))
                .xsmall()
                .label(mode.label());
            let button = if self.opt.cursor == mode {
                button.primary()
            } else {
                button.ghost()
            };
            cursor_row = cursor_row.child(button.on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.opt.cursor = mode;
                    this.invalidate_report();
                    cx.notify();
                },
            )));
        }

        let mut block = v_flex()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(section_title("同步批次", fg))
                    .child(
                        Button::new("adv-toggle")
                            .xsmall()
                            .ghost()
                            .label(if self.show_advanced {
                                "收起高级"
                            } else {
                                "高级选项"
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.show_advanced = !this.show_advanced;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(label("全局游标", muted))
                    .child(cursor_row),
            );

        if self.opt.cursor == CursorMode::Custom {
            block = block.child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.opt.global_start).small()))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.opt.global_end).small())),
            );
        }

        block = block.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(label("并发", muted))
                .child(div().w(px(56.)).child(Input::new(&self.opt.parallelism).small()))
                .child(label("批大小", muted))
                .child(div().w(px(76.)).child(Input::new(&self.opt.batch_size).small()))
                .child(label("上限", muted))
                .child(div().w(px(84.)).child(Input::new(&self.opt.max_batch_size).small()))
                .child(
                    Checkbox::new("adaptive")
                        .small()
                        .checked(self.opt.adaptive_batch)
                        .label("自适应")
                        .on_change(cx.listener(|this, checked, _, cx| {
                            this.opt.adaptive_batch = *checked;
                            this.invalidate_report();
                            cx.notify();
                        })),
                ),
        );

        if self.show_advanced {
            let toggles: [(&'static str, &str, bool); 9] = [
                ("create-table", "自动建表", self.opt.create_table),
                ("create-db", "自动建库", self.opt.create_database),
                ("strip-fk", "移除外键", self.opt.strip_foreign_keys),
                ("strip-ai", "移除自增", self.opt.strip_auto_increment),
                ("map-coll", "collation 映射", self.opt.map_collation),
                ("fk-off", "关闭外键检查", self.opt.disable_foreign_key_checks),
                ("truncate", "清空目标表", self.opt.truncate_target),
                ("relax-sql", "放宽 sql_mode", self.opt.relax_sql_mode),
                ("continue", "失败继续", self.opt.continue_on_error),
            ];
            let mut grid = v_flex().gap_1();
            for chunk in toggles.chunks(2) {
                let mut row = h_flex().gap_4();
                for (id, text, value) in chunk {
                    let id = id.to_string();
                    let key = id.clone();
                    row = row.child(
                        div().w(px(150.)).child(
                            Checkbox::new(format!("adv-{id}"))
                                .small()
                                .checked(*value)
                                .label(text.to_string())
                                .on_change(cx.listener(move |this, checked, _, cx| {
                                    this.set_toggle(&key, *checked);
                                    cx.notify();
                                })),
                        ),
                    );
                }
                grid = grid.child(row);
            }
            block = block.child(grid);
        }

        let _ = (primary, primary_fg);
        block.into_any_element()
    }

    fn set_toggle(&mut self, key: &str, value: bool) {
        match key {
            "create-table" => self.opt.create_table = value,
            "create-db" => self.opt.create_database = value,
            "strip-fk" => self.opt.strip_foreign_keys = value,
            "strip-ai" => self.opt.strip_auto_increment = value,
            "map-coll" => self.opt.map_collation = value,
            "fk-off" => self.opt.disable_foreign_key_checks = value,
            "truncate" => self.opt.truncate_target = value,
            "relax-sql" => self.opt.relax_sql_mode = value,
            "continue" => self.opt.continue_on_error = value,
            _ => {}
        }
    }

    // ------------------------------------------------------------ 待同步表

    fn selected_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let primary = cx.theme().primary;
        let primary_fg = cx.theme().primary_foreground;

        let mut list = v_flex().gap_1().px_2().pb_2();
        if self.tables.iter().all(|t| !t.selected) {
            list = list.child(label("在左侧勾选要同步的表", muted));
        }
        for (i, row) in self.tables.iter().enumerate().filter(|(_, t)| t.selected) {
            let name = row.name.clone();
            let edit_name = row.name.clone();
            let remove_name = row.name.clone();
            let target = row
                .over
                .dst_table
                .clone()
                .unwrap_or_else(|| row.name.clone());
            let cursor_text = match row.over.cursor {
                None => "跟随全局".to_string(),
                Some(c) => match c {
                    CursorStart::Full => "全量".into(),
                    CursorStart::Resume => "续跑".into(),
                    CursorStart::Custom => format!(
                        "区间 {} → {}",
                        row.over.start_id.clone().unwrap_or_else(|| "min".into()),
                        row.over.end_id.clone().unwrap_or_else(|| "max".into())
                    ),
                },
            };
            let truncate = row.over.truncate == Some(true);

            let mut item = h_flex()
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .child(div().flex_1().min_w_0().text_xs().child(format!("{name} → {target}")))
                .child(badge(
                    cursor_text,
                    if row.over.cursor.is_some() {
                        primary_fg
                    } else {
                        muted
                    },
                    if row.over.cursor.is_some() {
                        primary
                    } else {
                        border
                    },
                ));
            if truncate {
                item = item.child(badge("清空目标", primary_fg, cx.theme().danger));
            }
            item = item
                .child(
                    Button::new(format!("edit-{i}"))
                        .xsmall()
                        .ghost()
                        .label("区间")
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.start_edit(&edit_name, window, cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(format!("rm-{i}"))
                        .xsmall()
                        .ghost()
                        .label("移除")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.toggle_table(&remove_name, false);
                            cx.notify();
                        })),
                );
            list = list.child(item);
        }

        let mut head = h_flex()
            .items_center()
            .justify_between()
            .px_3()
            .pt_2()
            .pb_1()
            .child(section_title(
                format!("待同步 {} 张 · 估算 {} 行", self.selected_count(), human_rows(self.estimated_rows())),
                fg,
            ));

        let mut actions = h_flex().gap_2().items_center();
        actions = actions.child(
            Button::new("do-preflight")
                .xsmall()
                .label(if self.preflighting { "预检中…" } else { "预检" })
                .disabled(!self.can_preflight())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.do_preflight(cx);
                    cx.notify();
                })),
        );
        if self.running {
            actions = actions.child(
                Button::new("do-cancel")
                    .xsmall()
                    .danger()
                    .label("取消同步")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.do_cancel();
                        cx.notify();
                    })),
            );
        } else {
            actions = actions.child(
                Button::new("do-start")
                    .xsmall()
                    .primary()
                    .label("开始同步")
                    .disabled(!self.can_start())
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.do_start(cx);
                        cx.notify();
                    })),
            );
        }
        head = head.child(actions);

        v_flex()
            .flex_1()
            .min_h_0()
            .border_b_1()
            .border_color(border)
            .child(head)
            .child(
                div()
                    .id("selected-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(list),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ 单表覆盖编辑

    fn editor_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(name) = self.editing.clone() else {
            return div().into_any_element();
        };
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;

        let mut cursor_row = h_flex().gap_1().items_center().child(label("游标", muted));
        let options: [(Option<CursorStart>, &str); 4] = [
            (None, "跟随全局"),
            (Some(CursorStart::Full), "全量"),
            (Some(CursorStart::Resume), "续跑"),
            (Some(CursorStart::Custom), "自定义"),
        ];
        for (mode, text) in options {
            let button = Button::new(format!("em-{text}")).xsmall().label(text);
            let button = if self.ed_cursor == mode {
                button.primary()
            } else {
                button.ghost()
            };
            cursor_row = cursor_row.child(button.on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.ed_cursor = mode;
                    cx.notify();
                },
            )));
        }

        let show_range = self.ed_cursor == Some(CursorStart::Custom);
        let apply_label = format!("应用（{name}）");

        let mut block = v_flex()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(border)
            .child(section_title(format!("表级覆盖 · {name}"), fg))
            .child(cursor_row);

        if show_range {
            block = block.child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.ed_start).small()))
                    .child(div().flex_1().min_w_0().child(Input::new(&self.ed_end).small())),
            );
        }

        block = block
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.ed_dst_table).small()))
                    .child(
                        Button::new("ed-truncate")
                            .xsmall()
                            .ghost()
                            .label(format!("清空目标表：{}", self.ed_truncate.label()))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.ed_truncate = this.ed_truncate.next();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("ed-cancel")
                            .xsmall()
                            .ghost()
                            .label("取消")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("ed-apply")
                            .xsmall()
                            .primary()
                            .label(apply_label)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.apply_edit(cx);
                                cx.notify();
                            })),
                    ),
            );

        block.into_any_element()
    }

    // ------------------------------------------------------------ 进度与预检

    fn progress_block(&self, cx: &mut Context<Self>) -> AnyElement {
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;

        if self.progress.is_empty() && self.report.is_none() && self.logs.is_empty() {
            return div()
                .flex_shrink_0()
                .p_3()
                .child(label("预检或同步后，这里显示逐表进度与日志。", muted))
                .into_any_element();
        }

        let mut list = v_flex().gap_2().p_3();
        let mut events: Vec<(&String, &TableProgress)> = self.progress.iter().collect();
        events.sort_by(|a, b| a.0.cmp(b.0));
        for (name, p) in events {
            let color = match p.status {
                "failed" => danger,
                "done" => cx.theme().success,
                "running" => cx.theme().primary,
                _ => muted,
            };
            let mut row = v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xs().child(name.clone()))
                        .child(label(
                            format!(
                                "{} · {} / {} 行 · {:.0} 行/s{}",
                                p.status,
                                p.rows,
                                p.planned,
                                p.rps,
                                p.cursor
                                    .is_empty()
                                    .then(String::new)
                                    .unwrap_or_else(|| format!(" · {}", p.cursor))
                            ),
                            color,
                        )),
                )
                .child(
                    Progress::new(format!("pg-{name}"))
                        .value(p.percent())
                        .color(color),
                );
            if let Some(err) = &p.error {
                row = row.child(label(err.clone(), danger));
            }
            list = list.child(row);
        }

        // 预检问题
        if let Some(report) = &self.report {
            for t in &report.tables {
                for check in &t.checks {
                    let (color, text) = match check.severity {
                        Severity::Error => (danger, "错误"),
                        Severity::Warning => (warning, "警告"),
                        Severity::Info => (muted, "信息"),
                        Severity::Ok => continue,
                    };
                    list = list.child(label(
                        format!("[{}] {} · {text}：{}", t.table, check.code, check.message),
                        color,
                    ));
                }
            }
        }

        // 最近日志
        let recent: Vec<_> = self.logs.iter().rev().take(40).collect();
        if !recent.is_empty() {
            let mut log_list = v_flex().gap_1();
            for (level, msg) in recent.into_iter().rev() {
                log_list = log_list.child(label(format!("{level} · {msg}"), muted));
            }
            list = list.child(
                div()
                    .pt_1()
                    .border_t_1()
                    .border_color(border)
                    .child(log_list),
            );
        }

        v_flex()
            .flex_shrink_0()
            .h(px(230.))
            .min_h_0()
            .border_t_1()
            .border_color(border)
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .pt_2()
                    .child(section_title("运行进度", fg)),
            )
            .child(
                div()
                    .id("progress-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(list),
            )
            .into_any_element()
    }

    fn report_block(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let success = cx.theme().success;
        let warning = cx.theme().warning;

        let Some(report) = &self.report else {
            return v_flex()
                .gap_1()
                .child(label("尚未预检", muted))
                .into_any_element();
        };

        let mut block = v_flex().gap_1();
        for t in &report.tables {
            let (color, text) = if t.syncable {
                (
                    success,
                    format!(
                        "{} → {} · 预计 {} 行 (源 MAX {})",
                        t.table,
                        t.target_table,
                        t.planned_rows,
                        t.source_max_pk.clone().unwrap_or_else(|| "-".into())
                    ),
                )
            } else {
                let reason = t
                    .checks
                    .iter()
                    .find(|c| c.severity == Severity::Error)
                    .map(|c| c.message.clone())
                    .unwrap_or_else(|| "不可同步".into());
                (danger, format!("{} · {reason}", t.table))
            };
            block = block.child(label(text, color));
        }
        if report.tables.iter().any(|t| !t.syncable) {
            block = block.child(label("被阻塞的表可以取消勾选后重试", warning));
        }
        let _ = cx;
        block.into_any_element()
    }
}
