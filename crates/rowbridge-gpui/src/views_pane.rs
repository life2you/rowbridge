//! 三栏面板：连接、库表、批次选项、进度。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    checkbox::Checkbox, input::Input, scroll::ScrollableElement as _, Icon, IconName, *,
};
use gpui_kit::*;
use rowbridge_core::config::CursorStart;
use rowbridge_core::preflight::Severity;

use crate::app::RowBridgeApp;
use crate::model::*;
use crate::views::{card_head, chip, divider, field_label, label, mono, panel_card};

impl RowBridgeApp {
    pub(crate) fn side_pane(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let accent = cx.theme().accent;

        let (icon, connected) = match side {
            DbSide::Src => (IconName::Network, !self.src_schemas.is_empty()),
            DbSide::Dst => (IconName::HardDrive, !self.dst_schemas.is_empty()),
        };

        let mut head = card_head(side.title(), icon, muted, border);
        head = head
            .child(div().flex_1())
            .child(chip(
                if connected { "已连接" } else { "未连接" },
                if connected { success } else { muted },
                accent,
            ));

        let mut body = v_flex().flex_1().min_h_0().gap_3().p_3();
        body = body.child(self.conn_editor(side, cx));
        body = body.child(divider(border));
        body = body.child(self.schema_section(side, cx));
        body = body.child(divider(border));
        body = match side {
            DbSide::Src => body.child(self.table_section(cx)),
            DbSide::Dst => body.child(self.dst_section(cx)),
        };

        panel_card(card_bg, border)
            .w(px(304.))
            .h_full()
            .flex_shrink_0()
            .child(head)
            .child(body)
            .into_any_element()
    }

    // ------------------------------------------------------------ 连接

    fn conn_editor(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let panel = self.panel(side);
        let tag = match side {
            DbSide::Src => "src",
            DbSide::Dst => "dst",
        };

        let mut block = v_flex().flex_shrink_0().gap_2();

        // 已保存数据源
        block = block.child(
            v_flex()
                .gap_1()
                .child(field_label("已保存数据源", muted))
                .child(self.saved_chips(side, cx)),
        );

        // 主机 / 端口
        block = block.child(
            h_flex()
                .gap_2()
                .items_end()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(field_label("主机", muted))
                        .child(Input::new(&panel.host).small()),
                )
                .child(
                    v_flex()
                        .w(px(84.))
                        .gap_1()
                        .child(field_label("端口", muted))
                        .child(Input::new(&panel.port).small()),
                ),
        );

        // 用户 / 密码
        block = block.child(
            h_flex()
                .gap_2()
                .items_end()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(field_label("用户", muted))
                        .child(Input::new(&panel.user).small()),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(field_label("密码", muted))
                        .child(Input::new(&panel.password).small()),
                ),
        );

        // TLS + 动作
        block = block.child(
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
                        .ghost()
                        .icon(IconName::Network)
                        .label("测试")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.test_conn(side, cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(format!("load-{tag}"))
                        .xsmall()
                        .outline()
                        .icon(IconName::RefreshCw)
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
                        .icon(IconName::Plus)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.save_current(side, cx);
                            cx.notify();
                        })),
                ),
        );

        let note_color = match &panel.note {
            Some((true, _)) => success,
            Some((false, _)) => danger,
            None => muted,
        };
        let note_text = match &panel.note {
            Some((_, msg)) => msg.clone(),
            None => match (&panel.label, panel.busy) {
                (_, true) => "连接中…".into(),
                (Some(name), _) => format!("当前数据源：{name}").into(),
                (None, _) => "未选择已保存的数据源".into(),
            },
        };
        block
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .min_h(px(16.))
                    .child(Icon::new(IconName::CircleAlert).xsmall().text_color(note_color))
                    .child(label(note_text, note_color)),
            )
            .into_any_element()
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

    fn saved_chips(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let tag = match side {
            DbSide::Src => "src",
            DbSide::Dst => "dst",
        };
        let mut row = h_flex().gap_1().flex_wrap();
        if self.saved.is_empty() {
            row = row.child(label("暂无", muted));
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

    // ------------------------------------------------------------ 数据库列表

    fn schema_section(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let hover_bg = cx.theme().accent;
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
                .h(px(26.))
                .px_2()
                .rounded_md()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_xs()
                .child(Icon::new(IconName::Network).xsmall())
                .child(div().flex_1().min_w_0().child(name.clone()));
            row = if selected {
                row.bg(cx.theme().list_active)
                    .text_color(cx.theme().foreground)
                    .font_weight(FontWeight::MEDIUM)
            } else {
                row.text_color(muted).hover(move |s| s.bg(hover_bg))
            };
            if selected {
                row = row.child(Icon::new(IconName::Check).xsmall());
            }
            rows = rows.child(row.on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.choose_schema(side, &name, cx);
                    cx.notify();
                },
            )));
        }

        v_flex()
            .flex_shrink_0()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(field_label("数据库", muted))
                    .child(div().flex_1())
                    .child(label(format!("{} 个", self.schemas(side).len()), muted)),
            )
            .child(
                div()
                    .id(format!("schema-{:?}", side as u8))
                    .max_h(px(150.))
                    .overflow_y_scrollbar()
                    .rounded_md()
                    .child(rows),
            )
            .border_color(border)
            .into_any_element()
    }

    // ------------------------------------------------------------ 表清单

    fn table_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let accent = cx.theme().accent;
        let hover_bg = cx.theme().accent;
        let selected_bg = cx.theme().list_active;

        let mut head = h_flex()
            .items_center()
            .gap_2()
            .child(Icon::new(IconName::Frame).xsmall().text_color(muted))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("表 · 已选 {}", self.selected_count())),
            )
            .child(div().flex_1());

        head = head
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

        let mut list = v_flex().gap_0p5();
        if self.loading_tables {
            list = list.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Loader).xsmall().text_color(muted))
                    .child(label("加载表清单…", muted)),
            );
        } else if self.tables.is_empty() {
            list = list.child(label("选择数据库后自动加载表清单", muted));
        }
        for (i, row) in self.tables.iter().enumerate() {
            let name = row.name.clone();
            let toggle_name = row.name.clone();
            let selected = row.selected;
            let disabled = row.unsupported.is_some();
            let mut item = h_flex()
                .h(px(24.))
                .gap_2()
                .items_center()
                .px_1()
                .rounded_sm()
                .hover(move |s| s.bg(hover_bg))
                .child(
                    Checkbox::new(format!("tbl-{i}"))
                        .xsmall()
                        .checked(selected)
                        .disabled(disabled)
                        .on_change(cx.listener(move |this, checked, _, cx| {
                            this.toggle_table(&toggle_name, *checked);
                            cx.notify();
                        })),
                )
                .child(div().flex_1().min_w_0().text_xs().truncate().child(name.clone()))
                .child(
                    div()
                        .w(px(64.))
                        .text_right()
                        .child(mono(human_rows(row.rows_estimate), muted)),
                );

            match (&row.unsupported, &row.primary_key) {
                (Some(reason), _) => {
                    item = item.child(chip(reason.clone(), danger, accent));
                }
                (None, Some(pk)) => {
                    item = item.child(chip(format!("PK {pk}"), muted, accent));
                }
                _ => {}
            }
            if selected {
                item = item.bg(selected_bg);
            }
            list = list.child(item);
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(head)
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

    // ------------------------------------------------------------ 目标状态

    fn dst_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let warning = cx.theme().warning;
        let border = cx.theme().border;

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

        let stat = |name: &'static str, value: String, color: Hsla| -> AnyElement {
            h_flex()
                .justify_between()
                .items_center()
                .h(px(24.))
                .child(label(name, muted))
                .child(div().child(mono(value, color)))
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::ChartPie).xsmall().text_color(muted))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .child("目标状态"),
                    ),
            )
            .child(stat("将新建表", create.to_string(), warning))
            .child(stat("已有数据", existing.to_string(), success))
            .child(stat("待补列", alter.to_string(), warning))
            .child(divider(border))
            .child(
                Button::new("sync-schema")
                    .small()
                    .outline()
                    .icon(IconName::Replace)
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
            .child(
                div()
                    .id("report-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(self.report_block(cx)),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ 中间：批次

    pub(crate) fn center_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .flex_1()
            .min_w(px(430.))
            .h_full()
            .min_h_0()
            .gap_2()
            .child(self.batch_card(cx))
            .child(self.progress_card(cx))
            .into_any_element()
    }

    fn batch_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
        let fg = cx.theme().foreground;
        let accent = cx.theme().accent;

        let mut head = card_head("同步批次", IconName::Replace, muted, border);
        head = head.child(chip(
            format!(
                "已选 {} 张 · 估算 {} 行",
                self.selected_count(),
                human_rows(self.estimated_rows())
            ),
            muted,
            accent,
        ));

        let src_label = self
            .src_schema
            .clone()
            .unwrap_or_else(|| "源库".into());
        let dst_label = self
            .dst_schema
            .clone()
            .unwrap_or_else(|| "目标库".into());
        head = head
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(div().text_xs().text_color(muted).child(src_label))
                    .child(Icon::new(IconName::ArrowRight).xsmall().text_color(muted))
                    .child(div().text_xs().text_color(muted).child(dst_label)),
            )
            .child(div().flex_1());

        if self.running {
            head = head.child(
                Button::new("do-cancel")
                    .xsmall()
                    .danger()
                    .icon(IconName::Close)
                    .label("取消同步")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.do_cancel();
                        cx.notify();
                    })),
            );
        } else {
            head = head
                .child(
                    Button::new("do-preflight")
                        .xsmall()
                        .ghost()
                        .icon(IconName::Search)
                        .label(if self.preflighting { "预检中…" } else { "预检" })
                        .disabled(!self.can_preflight())
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.do_preflight(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("do-start")
                        .xsmall()
                        .primary()
                        .icon(IconName::Play)
                        .label("开始同步")
                        .disabled(!self.can_start())
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.do_start(cx);
                            cx.notify();
                        })),
                );
        }

        let mut body = v_flex().flex_1().min_h_0().gap_2().p_3();
        body = body
            .child(self.options_row(cx))
            .child(self.batch_row(cx));

        if self.show_advanced {
            body = body.child(self.advanced_grid(cx));
        }

        body = body
            .child(divider(border))
            .child(self.edit_hint(muted))
            .child(
                div()
                    .id("selected-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(self.selected_list(cx)),
            );

        if self.editing.is_some() {
            body = body.child(self.editor_panel(cx));
        }

        panel_card(card_bg, border)
            .flex_1()
            .min_h_0()
            .max_h(px(470.))
            .child(head)
            .child(body)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .px_3()
                    .pb_2()
                    .child(label("", fg)),
            )
            .into_any_element()
    }

    fn options_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().accent;

        let mut seg = h_flex().gap_1().p_1().rounded_md().bg(accent);
        for mode in [CursorMode::Full, CursorMode::Resume, CursorMode::Custom] {
            let button = Button::new(format!("gm-{}", mode.label()))
                .xsmall()
                .label(mode.label());
            let button = if self.opt.cursor == mode {
                button.primary()
            } else {
                button.ghost()
            };
            seg = seg.child(button.on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.opt.cursor = mode;
                    this.invalidate_report();
                    cx.notify();
                },
            )));
        }

        let mut row = h_flex()
            .gap_3()
            .items_center()
            .flex_shrink_0()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(field_label("全局游标", muted))
                    .child(seg),
            );

        if self.opt.cursor == CursorMode::Custom {
            row = row
                .child(
                    v_flex()
                        .gap_1()
                        .child(field_label("起始 ID（含）", muted))
                        .child(div().w(px(120.)).child(Input::new(&self.opt.global_start).small())),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(field_label("结束 ID（含）", muted))
                        .child(div().w(px(120.)).child(Input::new(&self.opt.global_end).small())),
                );
        }

        row.into_any_element()
    }

    fn batch_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;

        h_flex()
            .gap_3()
            .items_end()
            .flex_shrink_0()
            .child(
                v_flex()
                    .w(px(64.))
                    .gap_1()
                    .child(field_label("并发", muted))
                    .child(Input::new(&self.opt.parallelism).small()),
            )
            .child(
                v_flex()
                    .w(px(90.))
                    .gap_1()
                    .child(field_label("批大小", muted))
                    .child(Input::new(&self.opt.batch_size).small()),
            )
            .child(
                v_flex()
                    .w(px(96.))
                    .gap_1()
                    .child(field_label("批量上限", muted))
                    .child(Input::new(&self.opt.max_batch_size).small()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .h(px(30.))
                    .child(
                        Checkbox::new("adaptive")
                            .small()
                            .checked(self.opt.adaptive_batch)
                            .label("自适应批大小")
                            .on_change(cx.listener(|this, checked, _, cx| {
                                this.opt.adaptive_batch = *checked;
                                this.invalidate_report();
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("adv-toggle")
                    .xsmall()
                    .ghost()
                    .icon(if self.show_advanced {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .icon(IconName::Settings2)
                    .label("高级选项")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.show_advanced = !this.show_advanced;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn advanced_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let accent = cx.theme().accent;
        let toggles: [(&'static str, &str, bool); 9] = [
            ("create-table", "自动建表", self.opt.create_table),
            ("create-db", "自动建库", self.opt.create_database),
            ("strip-fk", "移除外键约束", self.opt.strip_foreign_keys),
            ("strip-ai", "移除自增属性", self.opt.strip_auto_increment),
            ("map-coll", "collation 映射", self.opt.map_collation),
            ("fk-off", "关闭外键检查", self.opt.disable_foreign_key_checks),
            ("truncate", "清空目标表", self.opt.truncate_target),
            ("relax-sql", "放宽 sql_mode", self.opt.relax_sql_mode),
            ("continue", "失败继续其它表", self.opt.continue_on_error),
        ];

        let mut grid = v_flex().gap_1();
        for chunk in toggles.chunks(3) {
            let mut row = h_flex().gap_4();
            for (id, text, value) in chunk {
                let id = id.to_string();
                let key = id.clone();
                row = row.child(
                    div().w(px(140.)).child(
                        Checkbox::new(format!("adv-{id}"))
                            .xsmall()
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

        div()
            .flex_shrink_0()
            .p_2()
            .rounded_md()
            .bg(accent)
            .child(grid)
            .into_any_element()
    }

    fn edit_hint(&self, muted: Hsla) -> AnyElement {
        let count = self.selected_count();
        let overridden = self
            .tables
            .iter()
            .filter(|t| t.selected && t.over.cursor.is_some())
            .count();
        let text = if count == 0 {
            "在左侧勾选要同步的表".to_string()
        } else if overridden == 0 {
            format!("{count} 张表跟随全局游标")
        } else {
            format!("{count} 张表 · {overridden} 张使用表级区间")
        };
        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .child(label(text, muted))
            .into_any_element()
    }

    fn selected_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let primary = cx.theme().primary;
        let primary_fg = cx.theme().primary_foreground;
        let accent = cx.theme().accent;
        let hover_bg = cx.theme().accent;
        let danger = cx.theme().danger;

        let planned_of = |name: &str| -> Option<u64> {
            self.report
                .as_ref()
                .and_then(|r| r.tables.iter().find(|t| t.table == name))
                .map(|t| t.planned_rows)
        };

        let mut list = v_flex().gap_1();

        if self.selected_count() > 0 {
            list = list.child(
                h_flex()
                    .h(px(18.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .text_color(muted)
                    .child(div().w(px(3.)))
                    .child(div().flex_1().child("表 → 目标表"))
                    .child(div().w(px(112.)).child("游标"))
                    .child(div().w(px(78.)).text_right().child("计划行数"))
                    .child(div().w(px(76.))),
            );
        }

        if self.selected_count() == 0 {
            list = list.child(
                v_flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .py_8()
                    .rounded_md()
                    .border_1()
                    .border_dashed()
                    .border_color(border)
                    .child(Icon::new(IconName::Frame).text_color(muted))
                    .child(div().text_xs().text_color(muted).child("还没有选中任何表"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("在左侧勾选要同步的表，再点「预检」"),
                    ),
            );
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
            let (cursor_text, cursor_color) = match row.over.cursor {
                None => ("跟随全局".to_string(), muted),
                Some(CursorStart::Full) => ("全量".to_string(), primary_fg),
                Some(CursorStart::Resume) => ("续跑".to_string(), primary_fg),
                Some(CursorStart::Custom) => (
                    format!(
                        "{} → {}",
                        row.over.start_id.clone().unwrap_or_else(|| "min".into()),
                        row.over.end_id.clone().unwrap_or_else(|| "max".into())
                    ),
                    primary_fg,
                ),
            };
            let has_override = row.over.cursor.is_some();

            let mut item = h_flex()
                .gap_2()
                .items_center()
                .h(px(30.))
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .hover(move |s| s.bg(hover_bg))
                .child(
                    div()
                        .w(px(3.))
                        .h(px(16.))
                        .rounded_full()
                        .bg(if has_override { primary } else { accent }),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .items_center()
                        .child(div().text_xs().truncate().child(name.clone()))
                        .child(Icon::new(IconName::ArrowRight).xsmall().text_color(muted))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(target.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(112.))
                        .child(chip(
                            cursor_text,
                            cursor_color,
                            if has_override { primary } else { accent },
                        )),
                )
                .child(
                    div().w(px(78.)).text_right().child(mono(
                        match planned_of(&row.name) {
                            Some(v) => v.to_string(),
                            None => "-".to_string(),
                        },
                        muted,
                    )),
                );

            if row.over.truncate == Some(true) {
                item = item.child(chip("清空目标", danger, accent));
            }

            item = item
                .child(
                    Button::new(format!("edit-{i}"))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Calendar)
                        .label("区间")
                        .tooltip("设置本表起止 ID / 目标表名")
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.start_edit(&edit_name, window, cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(format!("rm-{i}"))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Close)
                        .tooltip("从批次中移除")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.toggle_table(&remove_name, false);
                            cx.notify();
                        })),
                );
            list = list.child(item);
        }

        list.into_any_element()
    }

    fn editor_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let some_name = match self.editing.clone() {
            Some(n) => n,
            None => return div().into_any_element(),
        };
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().accent;

        let mut cursor_row = h_flex()
            .gap_1()
            .items_center()
            .child(field_label("本表游标", muted));
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

        let mut block = v_flex()
            .flex_shrink_0()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(accent)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::Calendar).xsmall().text_color(muted))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(format!("表级覆盖 · {some_name}")),
                    )
                    .child(div().flex_1())
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
                            .icon(IconName::Check)
                            .label("应用")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.apply_edit(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(cursor_row);

        if self.ed_cursor == Some(CursorStart::Custom) {
            block = block.child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(field_label("起始 ID（含）", muted))
                            .child(Input::new(&self.ed_start).small()),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(field_label("结束 ID（含）", muted))
                            .child(Input::new(&self.ed_end).small()),
                    ),
            );
        }

        block
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(field_label("目标表名（留空同名）", muted))
                            .child(Input::new(&self.ed_dst_table).small()),
                    )
                    .child(
                        Button::new("ed-truncate")
                            .xsmall()
                            .outline()
                            .icon(IconName::Delete)
                            .label(format!("清空目标：{}", self.ed_truncate.label()))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.ed_truncate = this.ed_truncate.next();
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ 运行状态

    fn progress_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
        let fg = cx.theme().foreground;
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;
        let success = cx.theme().success;

        let total_done: u64 = self.progress.values().map(|p| p.rows).sum();
        let planned: u64 = self.progress.values().map(|p| p.planned).sum();
        let done_tables = self
            .progress
            .values()
            .filter(|p| p.status == "done")
            .count();

        let mut head = card_head("运行状态", IconName::ChartPie, muted, border);
        head = head.child(div().flex_1());
        if !self.progress.is_empty() {
            head = head
                .child(chip(
                    format!("{done_tables}/{} 张完成", self.progress.len()),
                    muted,
                    cx.theme().accent,
                ))
                .child(chip(
                    format!("{} / {} 行", total_done, planned),
                    fg,
                    cx.theme().accent,
                ));
        }

        let mut list = v_flex().gap_2().p_3();

        if self.progress.is_empty() && self.report.is_none() && self.logs.is_empty() {
            list = list.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::CircleAlert).xsmall().text_color(muted))
                    .child(label("预检或同步后，这里显示逐表进度与日志", muted)),
            );
        }

        let mut rows: Vec<(&String, &TableProgress)> = self.progress.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        for (name, p) in rows {
            let color = match p.status {
                "failed" => danger,
                "done" => success,
                "running" => cx.theme().info,
                _ => muted,
            };
            let status_text = match p.status {
                "failed" => "失败",
                "done" => "完成",
                "running" => "同步中",
                _ => "待开始",
            };
            let track = cx.theme().accent;
            list = list.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(140.))
                                    .text_xs()
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(track)
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(p.percent() / 100.0))
                                            .rounded_full()
                                            .bg(color),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(104.))
                                    .text_right()
                                    .child(mono(format!("{} / {}", p.rows, p.planned), muted)),
                            )
                            .child(chip(status_text, color, track)),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .pl(px(140.))
                            .child(mono(format!("{:.0} 行/s", p.rps), muted))
                            .children(if p.cursor.is_empty() {
                                None
                            } else {
                                Some(mono(format!("游标 {}", p.cursor), muted))
                            }),
                    ),
            );
            if let Some(err) = &p.error {
                list = list.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::CircleX).xsmall().text_color(danger))
                        .child(label(err.clone(), danger)),
                );
            }
        }

        if let Some(report) = &self.report {
            for t in &report.tables {
                for check in &t.checks {
                    let (color, icon, text) = match check.severity {
                        Severity::Error => (danger, IconName::CircleX, "错误"),
                        Severity::Warning => (warning, IconName::TriangleAlert, "警告"),
                        Severity::Info => (muted, IconName::CircleAlert, "信息"),
                        Severity::Ok => continue,
                    };
                    list = list.child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(Icon::new(icon).xsmall().text_color(color))
                            .child(label(
                                format!("{} · {text}：{}", t.table, check.message),
                                color,
                            )),
                    );
                }
            }
        }

        let recent: Vec<_> = self.logs.iter().rev().take(20).collect();
        if !recent.is_empty() {
            let mut log_list = v_flex().gap_1();
            for (level, msg) in recent.into_iter().rev() {
                let color = match level.as_ref() {
                    "error" => danger,
                    "warn" => warning,
                    _ => muted,
                };
                log_list = log_list.child(label(format!("{level} · {msg}"), color));
            }
            list = list
                .child(divider(border))
                .child(log_list);
        }

        panel_card(card_bg, border)
            .flex_1()
            .min_h(px(210.))
            .min_h_0()
            .child(head)
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

        let Some(report) = &self.report else {
            return h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(IconName::CircleAlert).xsmall().text_color(muted))
                .child(label("尚未预检", muted))
                .into_any_element();
        };

        let mut block = v_flex().gap_1();
        for t in &report.tables {
            let (color, icon, text) = if t.syncable {
                (
                    success,
                    IconName::CircleCheck,
                    format!(
                        "{} → {} · 预计 {} 行",
                        t.table, t.target_table, t.planned_rows
                    ),
                )
            } else {
                let reason = t
                    .checks
                    .iter()
                    .find(|c| c.severity == Severity::Error)
                    .map(|c| c.message.clone())
                    .unwrap_or_else(|| "不可同步".into());
                (danger, IconName::CircleX, format!("{} · {reason}", t.table))
            };
            block = block.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(Icon::new(icon).xsmall().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .line_clamp(2)
                            .text_color(color)
                            .child(text),
                    ),
            );
        }
        let _ = cx;
        block.into_any_element()
    }
}
