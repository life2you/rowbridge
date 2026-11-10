//! 三栏面板：连接、库表、批次选项、进度。

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{
    checkbox::Checkbox, input::Input, scroll::ScrollableElement as _, Icon, IconName,
    WindowExt as _, *,
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
        head = head.child(div().flex_1()).child(chip(
            if connected { "已连接" } else { "未连接" },
            if connected { success } else { muted },
            accent,
        ));

        let mut body = v_flex().flex_1().min_h_0().gap_3().p_3();
        body = body.child(self.source_selector(side, cx));
        body = body.child(self.schema_selector(side, cx));
        body = body.child(divider(border));
        body = match side {
            DbSide::Src => body.child(self.table_section(cx)),
            DbSide::Dst => body.child(self.dst_section(cx)),
        };

        panel_card(card_bg, border)
            .w(px(320.))
            .h_full()
            .flex_shrink_0()
            .child(head)
            .child(body)
            .into_any_element()
    }

    // ------------------------------------------------------------ 数据源 & 数据库下拉选择

    fn source_selector(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let tag = match side {
            DbSide::Src => "src",
            DbSide::Dst => "dst",
        };
        let panel = self.panel(side);
        let current_saved_id = panel.saved_id.clone();
        let current_conn = self
            .saved
            .iter()
            .find(|c| Some(&c.id) == current_saved_id.as_ref());

        let view_entity = cx.entity().clone();
        let saved_list = self.saved.clone();

        let display_label = if let Some(conn) = current_conn {
            conn.name.clone()
        } else {
            "选择数据源…".to_string()
        };

        let btn = Button::new(format!("source-dropdown-{tag}"))
            .w_full()
            .outline()
            .dropdown_caret(true)
            .label(display_label);

        let dropdown = btn.dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            let view_entity = view_entity.clone();
            let saved_list = saved_list.clone();
            let current_saved_id = current_saved_id.clone();
            let mut menu = menu.min_w(px(260.));
            if saved_list.is_empty() {
                menu = menu.item(PopupMenuItem::new("（暂无已保存数据源）").disabled(true));
            } else {
                for conn in &saved_list {
                    let id = conn.id.clone();
                    let name = conn.name.clone();
                    let addr = format!("{}:{}", conn.host, conn.port);
                    let checked = current_saved_id.as_deref() == Some(&id);
                    let view_entity = view_entity.clone();
                    menu = menu.item(
                        PopupMenuItem::new(format!("{name} ({addr})"))
                            .checked(checked)
                            .on_click(move |_, window, cx| {
                                view_entity.update(cx, |this, cx| {
                                    this.pick_saved(side, &id, window, cx);
                                    cx.notify();
                                });
                            }),
                    );
                }
            }
            menu = menu
                .separator()
                .item(PopupMenuItem::new("管理数据源…").on_click(move |_, _, cx| {
                    view_entity.update(cx, |this, cx| {
                        this.view = Tab::Sources;
                        cx.notify();
                    });
                }));
            menu
        });

        let mut col = v_flex().gap_1();
        col = col.child(
            h_flex()
                .items_center()
                .justify_between()
                .child(field_label("数据源", muted))
                .child(
                    Button::new(format!("manage-sources-{tag}"))
                        .xsmall()
                        .ghost()
                        .label("管理")
                        .tooltip("前往数据源管理页")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = Tab::Sources;
                            cx.notify();
                        })),
                ),
        );
        col = col.child(dropdown);
        if let Some((false, err)) = &panel.note {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(danger)
                    .truncate()
                    .child(err.clone()),
            );
        }
        col.into_any_element()
    }

    fn schema_selector(&self, side: DbSide, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let tag = match side {
            DbSide::Src => "src",
            DbSide::Dst => "dst",
        };
        let panel = self.panel(side);
        let is_busy = panel.busy;
        let current_schema = self.schema(side);
        let schemas = self.schemas(side).to_vec();
        let has_schemas = !schemas.is_empty();

        let view_entity = cx.entity().clone();

        let display_label = if is_busy {
            "加载中…".to_string()
        } else if let Some(schema) = current_schema.clone() {
            schema
        } else if has_schemas {
            "选择数据库…".to_string()
        } else {
            "暂无数据库".to_string()
        };

        let btn = Button::new(format!("schema-dropdown-{tag}"))
            .w_full()
            .outline()
            .dropdown_caret(true)
            .disabled(is_busy || !has_schemas)
            .label(display_label);

        let dropdown = btn.dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
            let view_entity = view_entity.clone();
            let schemas = schemas.clone();
            let current_schema = current_schema.clone();
            let mut menu = menu.min_w(px(240.)).max_h(px(320.)).scrollable(true);
            for s in &schemas {
                let name = s.name.clone();
                let checked = current_schema.as_deref() == Some(&name);
                let view_entity = view_entity.clone();
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                    move |_, _, cx| {
                        view_entity.update(cx, |this, cx| {
                            this.choose_schema(side, &name, cx);
                            cx.notify();
                        });
                    },
                ));
            }
            menu
        });

        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(field_label("数据库", muted))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .children(if !self.schemas(side).is_empty() {
                                Some(label(format!("{} 个", self.schemas(side).len()), muted))
                            } else {
                                None
                            })
                            .child(
                                Button::new(format!("refresh-schemas-{tag}"))
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::RefreshCw)
                                    .tooltip("重新加载数据库列表")
                                    .disabled(is_busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.load_schemas(side, cx);
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(dropdown)
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

    // ------------------------------------------------------------ 表清单

    fn table_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let accent = cx.theme().accent;
        let hover_bg = cx.theme().accent;
        let selected_bg = cx.theme().list_active;

        let filtered = self.filtered_tables(cx);
        let filter_str = self.table_filter.read(cx).value().trim().to_string();
        let is_filtered = !filter_str.is_empty();

        let count_label = if is_filtered {
            format!("已选 {} / 匹配 {}", self.selected_count(), filtered.len())
        } else {
            format!("已选 {} / 共 {}", self.selected_count(), self.tables.len())
        };

        let mut head = h_flex()
            .items_center()
            .gap_2()
            .child(Icon::new(IconName::Frame).xsmall().text_color(muted))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child(count_label),
            )
            .child(div().flex_1());

        head = head
            .child(
                Button::new("sel-all")
                    .xsmall()
                    .ghost()
                    .label(if is_filtered {
                        "全选匹配"
                    } else {
                        "全选"
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.select_filtered_tables(true, cx);
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

        let search_bar = h_flex().items_center().gap_1().child(
            div()
                .flex_1()
                .child(Input::new(&self.table_filter).small().cleanable(true)),
        );

        let filtered_rows: Vec<(usize, TableRow)> =
            filtered.into_iter().map(|(i, r)| (i, r.clone())).collect();
        let row_count = filtered_rows.len();

        let content: AnyElement = if self.loading_tables {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(IconName::Loader).xsmall().text_color(muted))
                        .child(label("加载表清单…", muted)),
                )
                .into_any_element()
        } else if self.tables.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .child(label("选择数据库后自动加载表清单", muted))
                .into_any_element()
        } else if row_count == 0 && is_filtered {
            div()
                .flex_1()
                .min_h_0()
                .child(label("没有匹配的表", muted))
                .into_any_element()
        } else {
            let scroll_handle = self.table_list_scroll.clone();
            div()
                .id("table-scroll-wrap")
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list(
                        "table-uniform-list",
                        row_count,
                        cx.processor(
                            move |_this, visible_range: std::ops::Range<usize>, _window, cx| {
                                let mut items = Vec::with_capacity(visible_range.len());
                                for row_ix in visible_range {
                                    let Some((i, row)) = filtered_rows.get(row_ix) else {
                                        continue;
                                    };
                                    let i = *i;
                                    let name = row.name.clone();
                                    let toggle_name = row.name.clone();
                                    let selected = row.selected;
                                    let disabled = row.unsupported.is_some();
                                    let mut item = h_flex()
                                        .w_full()
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
                                                .on_change(cx.listener(
                                                    move |this, checked, _, cx| {
                                                        this.toggle_table(&toggle_name, *checked);
                                                        cx.notify();
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_xs()
                                                .truncate()
                                                .child(name.clone()),
                                        )
                                        .child(
                                            div()
                                                .w(px(64.))
                                                .text_right()
                                                .child(mono(human_rows(row.rows_estimate), muted)),
                                        );

                                    match (&row.unsupported, &row.primary_key) {
                                        (Some(reason), _) => {
                                            let short = if reason.starts_with("复合主键") {
                                                "复合主键"
                                            } else if reason.starts_with("非整型主键") {
                                                "非整型主键"
                                            } else {
                                                reason.as_str()
                                            };
                                            item = item.child(chip(short, danger, accent));
                                        }
                                        (None, Some(pk)) => {
                                            item =
                                                item.child(chip(format!("PK {pk}"), muted, accent));
                                        }
                                        _ => {}
                                    }
                                    if selected {
                                        item = item.bg(selected_bg);
                                    }
                                    items.push(item);
                                }
                                items
                            },
                        ),
                    )
                    .size_full()
                    .track_scroll(&scroll_handle),
                )
                .vertical_scrollbar(&scroll_handle)
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(head)
            .child(search_bar)
            .child(content)
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

        let schema_btn_label = if self.syncing_schema {
            "同步结构中…".to_string()
        } else if !has_schema_work {
            if self.report.is_some() {
                "表结构已一致 (无需同步)".to_string()
            } else {
                "同步表结构 (需先预检)".to_string()
            }
        } else {
            let mut parts = Vec::new();
            if create > 0 {
                parts.push(format!("建表 {create}"));
            }
            if alter > 0 {
                parts.push(format!("补列 {alter}"));
            }
            format!("同步表结构 ({})", parts.join(" · "))
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
                    .label(schema_btn_label)
                    .disabled(!has_schema_work || self.syncing_schema)
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
        // 同步页只负责「组批次」：任务一旦开始就交给「进行中」，
        // 这里不再渲染任何运行中任务的进度/日志，避免阻塞下一个任务的创建。
        v_flex()
            .flex_1()
            .min_w(px(430.))
            .h_full()
            .min_h_0()
            .gap_2()
            .child(self.batch_card(cx))
            .into_any_element()
    }

    fn batch_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
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

        let src_label = self.src_schema.clone().unwrap_or_else(|| "源库".into());
        let dst_label = self.dst_schema.clone().unwrap_or_else(|| "目标库".into());
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
                        .label("加入任务并开始")
                        .disabled(!self.can_start())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            let to_truncate = this.will_truncate_any();
                            if !to_truncate.is_empty() {
                                let entity = cx.entity().clone();
                                let dst_schema = this.dst_schema.clone().unwrap_or_default();
                                let dst_host = this.dst.host_text(cx);
                                let dst_port = this.dst.port_text(cx);
                                let count = to_truncate.len();
                                let preview_names = to_truncate.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
                                let tables_str = if count > 8 {
                                    format!("{preview_names} 等共 {count} 张表")
                                } else {
                                    preview_names
                                };
                                window.open_alert_dialog(cx, move |alert, _w, _cx| {
                                    let entity = entity.clone();
                                    alert
                                        .confirm()
                                        .title("⚠️ 高风险确认：清空目标表")
                                        .description(format!(
                                            "以下目标表将在数据同步前被【清空】(TRUNCATE TABLE)：\n\n\
                                             目标实例：{dst_host}:{dst_port}\n\
                                             目标数据库：{dst_schema}\n\
                                             清空表：{tables_str}\n\n\
                                             此操作将永久清空目标表已有数据，且不可撤销！确定要继续吗？"
                                        ))
                                        .ok_variant(ButtonVariant::Danger)
                                        .ok_text("确认清空并开始")
                                        .cancel_text("取消")
                                        .on_ok(move |_e, _w, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.do_start(cx);
                                            });
                                            true
                                        })
                                });
                            } else {
                                this.do_start(cx);
                            }
                        cx.notify();
                    })),
            );

        let mut body = v_flex().flex_1().min_h_0().gap_2().p_3();
        body = body.child(self.batch_row(cx));

        if self.show_advanced {
            body = body.child(self.advanced_grid(cx));
        }

        let selected_count = self.selected_count();
        let valid_count = self
            .tables
            .iter()
            .filter(|t| t.unsupported.is_none())
            .count();
        let planned_total: u64 = self
            .tables
            .iter()
            .filter(|t| t.selected)
            .map(|t| {
                self.report
                    .as_ref()
                    .and_then(|r| r.tables.iter().find(|rt| rt.table == t.name))
                    .map(|rt| rt.planned_rows)
                    .unwrap_or(t.rows_estimate)
            })
            .sum();

        let footer = h_flex()
            .h(px(32.))
            .px_2()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(muted)
            .border_t_1()
            .border_color(border)
            .child(div().child(format!(
                "已选择 {}/{} 张表 · 预计总行数 {} 行",
                selected_count,
                valid_count,
                human_rows(planned_total)
            )))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("btn-clear-sel")
                            .xsmall()
                            .ghost()
                            .label("清空选择")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select_all_tables(false);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("btn-reset-overrides")
                            .xsmall()
                            .ghost()
                            .label("重置所有表配置")
                            .tooltip("清除所有表的表级区间、过滤条件和高级覆盖")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_all_overrides();
                                cx.notify();
                            })),
                    ),
            );

        let scroll_elem = if self.editing.is_some() {
            div()
                .id("selected-scroll")
                .h(px(100.))
                .flex_shrink_0()
                .overflow_y_scrollbar()
                .child(self.selected_list(cx))
        } else {
            div()
                .id("selected-scroll")
                .flex_1()
                .min_h(px(100.))
                .overflow_y_scrollbar()
                .child(self.selected_list(cx))
        };

        body = body.child(divider(border)).child(scroll_elem).child(footer);

        if self.editing.is_some() {
            body = body.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(self.editor_panel(cx)),
            );
        }

        panel_card(card_bg, border)
            .flex_1()
            .min_h_0()
            .child(head)
            .child(body)
            .into_any_element()
    }

    #[allow(dead_code)]
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
            seg = seg.child(
                button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.opt.cursor = mode;
                    this.invalidate_report();
                    cx.notify();
                })),
            );
        }

        let mut row = h_flex().gap_3().items_center().flex_shrink_0().child(
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
                        .child(
                            div()
                                .w(px(120.))
                                .child(Input::new(&self.opt.global_start).small()),
                        ),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(field_label("结束 ID（含）", muted))
                        .child(
                            div()
                                .w(px(120.))
                                .child(Input::new(&self.opt.global_end).small()),
                        ),
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
                h_flex().gap_2().items_center().h(px(30.)).child(
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
        let danger = cx.theme().danger;
        let toggles: [(&'static str, &str, bool); 9] = [
            ("create-table", "自动建表", self.opt.create_table),
            ("create-db", "自动建库", self.opt.create_database),
            ("strip-fk", "移除外键约束", self.opt.strip_foreign_keys),
            ("strip-ai", "移除自增属性", self.opt.strip_auto_increment),
            ("map-coll", "collation 映射", self.opt.map_collation),
            (
                "fk-off",
                "关闭外键检查",
                self.opt.disable_foreign_key_checks,
            ),
            ("truncate", "⚠️ 清空目标表", self.opt.truncate_target),
            ("relax-sql", "放宽 sql_mode", self.opt.relax_sql_mode),
            ("continue", "失败继续其它表", self.opt.continue_on_error),
        ];

        let mut grid = v_flex().gap_1();
        for chunk in toggles.chunks(3) {
            let mut row = h_flex().gap_4();
            for (id, text, value) in chunk {
                let id = id.to_string();
                let key = id.clone();
                let is_truncate = id == "truncate";
                let cb = Checkbox::new(format!("adv-{id}"))
                    .xsmall()
                    .checked(*value)
                    .label(text.to_string())
                    .on_change(cx.listener(move |this, checked, _, cx| {
                        this.set_toggle(&key, *checked);
                        this.invalidate_report();
                        cx.notify();
                    }));
                let el = if is_truncate && *value {
                    div()
                        .w(px(140.))
                        .px_1()
                        .py_0p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(danger)
                        .child(cb)
                } else {
                    div().w(px(140.)).child(cb)
                };
                row = row.child(el);
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

    fn selected_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let primary = cx.theme().primary;
        let accent = cx.theme().accent;
        let hover_bg = cx.theme().list_hover;
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;
        let fg = cx.theme().foreground;

        let planned_of = |name: &str| -> Option<u64> {
            self.report
                .as_ref()
                .and_then(|r| r.tables.iter().find(|t| t.table == name))
                .map(|t| t.planned_rows)
        };

        // 预检是否拿到了源库精确行数；拿不到时界面用 ≈ 标记为估算值。
        let planned_exact_of = |name: &str| -> bool {
            self.report
                .as_ref()
                .and_then(|r| r.tables.iter().find(|t| t.table == name))
                .map(|t| t.planned_rows_exact)
                .unwrap_or(false)
        };

        let mut list = v_flex().gap_1();

        if self.selected_count() == 0 {
            return list
                .child(
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
                )
                .into_any_element();
        }

        // 表头
        let valid_count = self
            .tables
            .iter()
            .filter(|t| t.unsupported.is_none())
            .count();
        let all_selected = self.selected_count() > 0 && self.selected_count() == valid_count;

        let header = h_flex()
            .h(px(28.))
            .px_2()
            .gap_1()
            .items_center()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(muted)
            .border_b_1()
            .border_color(border)
            .bg(accent)
            .child(
                div().w(px(18.)).flex_shrink_0().child(
                    Checkbox::new("th-chk-all")
                        .checked(all_selected)
                        .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                            this.select_all_tables(*checked);
                            cx.notify();
                        })),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w(px(100.))
                    .gap_1()
                    .items_center()
                    .child("表 ↑"),
            )
            .child(div().w(px(38.)).flex_shrink_0().child("主键"))
            .child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .text_right()
                    .child("行数(估)"),
            )
            .child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .text_right()
                    .child("源 MAX"),
            )
            .child(
                div()
                    .w(px(74.))
                    .flex_shrink_0()
                    .justify_center()
                    .child("模式"),
            )
            .child(
                div()
                    .w(px(50.))
                    .flex_shrink_0()
                    .justify_center()
                    .child("起始 ID"),
            )
            .child(
                div()
                    .w(px(50.))
                    .flex_shrink_0()
                    .justify_center()
                    .child("结束 ID"),
            )
            .child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .text_right()
                    .child("预计行数"),
            )
            .child(div().w(px(52.)).flex_shrink_0().text_right().child(""));

        list = list.child(header);

        for (i, row) in self.tables.iter().enumerate().filter(|(_, t)| t.selected) {
            let name = row.name.clone();
            let edit_name = row.name.clone();
            let remove_name = row.name.clone();

            let mode_label = match row.over.cursor {
                None | Some(CursorStart::Full) => "全量",
                Some(CursorStart::Resume) => "续跑",
                Some(CursorStart::Custom) => "自定义",
            };

            let start_disp = row.over.start_id.as_deref().unwrap_or("-");
            let end_disp = row.over.end_id.as_deref().unwrap_or("不限");

            let has_filter = row
                .over
                .filter
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let has_override = row.over.has_any();

            let planned = planned_of(&row.name).unwrap_or(row.rows_estimate);
            let planned_exact = planned_exact_of(&row.name);

            let view_entity = cx.entity().clone();
            let tbl_name_for_menu = name.clone();

            let mode_dropdown = Button::new(format!("tbl-mode-{i}"))
                .xsmall()
                .outline()
                .dropdown_caret(true)
                .label(mode_label)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let view_entity = view_entity.clone();
                    let tname = tbl_name_for_menu.clone();
                    menu.item(PopupMenuItem::new("全量").on_click({
                        let ve = view_entity.clone();
                        let tn = tname.clone();
                        move |_, _, cx| {
                            ve.update(cx, |this, cx| {
                                this.set_table_cursor(&tn, Some(CursorStart::Full));
                                cx.notify();
                            });
                        }
                    }))
                    .item(PopupMenuItem::new("续跑").on_click({
                        let ve = view_entity.clone();
                        let tn = tname.clone();
                        move |_, _, cx| {
                            ve.update(cx, |this, cx| {
                                this.set_table_cursor(&tn, Some(CursorStart::Resume));
                                cx.notify();
                            });
                        }
                    }))
                    .item(PopupMenuItem::new("自定义").on_click({
                        let ve = view_entity.clone();
                        let tn = tname.clone();
                        move |_, window, cx| {
                            ve.update(cx, |this, cx| {
                                this.set_table_cursor(&tn, Some(CursorStart::Custom));
                                this.start_edit(&tn, window, cx);
                                cx.notify();
                            });
                        }
                    }))
                });

            let mut row_item = h_flex()
                .gap_1()
                .items_center()
                .h(px(32.))
                .px_2()
                .rounded_md()
                .border_b_1()
                .border_color(border)
                .hover(move |s| s.bg(hover_bg));

            // 1. 复选框
            row_item = row_item.child(
                div().w(px(18.)).flex_shrink_0().child(
                    Checkbox::new(format!("tbl-chk-{i}"))
                        .checked(true)
                        .on_change(cx.listener({
                            let remove_name = remove_name.clone();
                            move |this, checked: &bool, _, cx| {
                                this.toggle_table(&remove_name, *checked);
                                cx.notify();
                            }
                        })),
                ),
            );

            // 2. 表名
            let mut name_cell = h_flex()
                .flex_1()
                .min_w(px(100.))
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(name.clone()),
                );

            if has_filter {
                name_cell = name_cell.child(chip("过滤", warning, accent));
            }

            if row.over.truncate == Some(true) {
                name_cell = name_cell.child(chip("清空", danger, accent));
            }

            row_item = row_item.child(name_cell);

            // 3. 主键
            row_item = row_item.child(
                div()
                    .w(px(38.))
                    .flex_shrink_0()
                    .child(mono(row.primary_key.as_deref().unwrap_or("-"), muted)),
            );

            // 4. 行数(估)
            row_item = row_item.child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .text_right()
                    .child(mono(human_rows(row.rows_estimate), muted)),
            );

            // 5. 源 MAX
            row_item = row_item.child(
                div()
                    .w(px(52.))
                    .flex_shrink_0()
                    .text_right()
                    .child(mono(row.source_max_pk.as_deref().unwrap_or("-"), muted)),
            );

            // 6. 模式下拉框
            row_item = row_item.child(div().w(px(74.)).flex_shrink_0().child(mode_dropdown));

            // 7. 起始 ID
            let edit_n1 = edit_name.clone();
            row_item = row_item.child(
                div().w(px(50.)).flex_shrink_0().child(
                    Button::new(format!("btn-start-{i}"))
                        .w_full()
                        .xsmall()
                        .outline()
                        .label(start_disp)
                        .tooltip("点击设置起始 ID")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_edit(&edit_n1, window, cx);
                            cx.notify();
                        })),
                ),
            );

            // 8. 结束 ID
            let edit_n2 = edit_name.clone();
            row_item = row_item.child(
                div().w(px(50.)).flex_shrink_0().child(
                    Button::new(format!("btn-end-{i}"))
                        .w_full()
                        .xsmall()
                        .outline()
                        .label(end_disp)
                        .tooltip("点击设置结束 ID")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_edit(&edit_n2, window, cx);
                            cx.notify();
                        })),
                ),
            );

            // 9. 预计行数（无 ≈ 前缀表示源库精确统计值）
            row_item = row_item.child(div().w(px(52.)).flex_shrink_0().text_right().child(mono(
                format!(
                    "{}{}",
                    if planned_exact { "" } else { "≈" },
                    human_rows(planned)
                ),
                fg,
            )));

            // 10. 操作：设置齿轮 + 移除
            let edit_n3 = edit_name.clone();
            let setting_btn = Button::new(format!("cfg-{i}"))
                .xsmall()
                .ghost()
                .icon(IconName::Settings)
                .tooltip("表级高级设置 / 字段条件过滤")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.start_edit(&edit_n3, window, cx);
                    cx.notify();
                }));
            let setting_btn = if has_override {
                setting_btn.text_color(primary)
            } else {
                setting_btn
            };

            let remove_btn = Button::new(format!("rm-{i}"))
                .xsmall()
                .ghost()
                .icon(IconName::Close)
                .tooltip("从本次同步批次中移除")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_table(&remove_name, false);
                    cx.notify();
                }));

            row_item = row_item.child(
                h_flex()
                    .w(px(52.))
                    .flex_shrink_0()
                    .gap_0p5()
                    .justify_end()
                    .items_center()
                    .child(setting_btn)
                    .child(remove_btn),
            );

            list = list.child(row_item);
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
        let active_cursor = self.ed_cursor.unwrap_or(CursorStart::Full);
        let options: [(CursorStart, &str); 3] = [
            (CursorStart::Full, "全量"),
            (CursorStart::Resume, "续跑"),
            (CursorStart::Custom, "自定义"),
        ];
        for (mode, text) in options {
            let button = Button::new(format!("em-{text}")).xsmall().label(text);
            let button = if active_cursor == mode {
                button.primary()
            } else {
                button.ghost()
            };
            cursor_row = cursor_row.child(button.on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.ed_cursor = Some(mode);
                    cx.notify();
                },
            )));
        }

        let mut block = v_flex()
            .flex_shrink_0()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(accent)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::Settings).small().text_color(muted))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(format!("表级设置 · {some_name}")),
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
                            .label("应用设置")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.apply_edit(cx);
                                cx.notify();
                            })),
                    ),
            );

        // 1. 其他字段条件过滤 + 字段下拉选择器
        let view_entity = cx.entity().clone();
        let cols_for_menu = self.ed_columns.clone();
        let loading_cols = self.loading_ed_columns;
        let col_btn_label = if loading_cols {
            "加载字段…".to_string()
        } else if cols_for_menu.is_empty() {
            "插入表字段".to_string()
        } else {
            format!("插入表字段 ({})", cols_for_menu.len())
        };

        let col_dropdown = Button::new("ed-col-dropdown")
            .xsmall()
            .outline()
            .dropdown_caret(true)
            .label(col_btn_label)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = menu.min_w(px(220.)).max_h(px(280.)).scrollable(true);
                if loading_cols {
                    menu = menu.item(PopupMenuItem::new("正在读取表字段…").disabled(true));
                } else if cols_for_menu.is_empty() {
                    menu = menu.item(PopupMenuItem::new("暂无字段信息").disabled(true));
                } else {
                    for col in &cols_for_menu {
                        let col_name = col.name.clone();
                        let col_type = col.column_type.clone();
                        let is_pk = col.is_primary;
                        let snippet = format!("`{col_name}` = ");
                        let ve = view_entity.clone();
                        let label_str = if is_pk {
                            format!("{col_name}  ({col_type} · PK)")
                        } else {
                            format!("{col_name}  ({col_type})")
                        };
                        menu = menu.item(PopupMenuItem::new(label_str).on_click(
                            move |_, window, cx| {
                                let snippet = snippet.clone();
                                ve.update(cx, |this, cx| {
                                    this.insert_filter_snippet(&snippet, window, cx);
                                    cx.notify();
                                });
                            },
                        ));
                    }
                }
                menu
            });

        block = block.child(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(field_label("字段条件过滤（SQL WHERE 表达式）", muted))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("如: status = 1 AND is_deleted = 0（留空不过滤）"),
                                ),
                        )
                        .child(col_dropdown),
                )
                .child(Input::new(&self.ed_filter).small().cleanable(true)),
        );

        // 2. 游标模式与起止 ID
        block = block.child(cursor_row);

        if active_cursor == CursorStart::Custom {
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

        let opt_row_1 = h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("ed-truncate")
                    .xsmall()
                    .outline()
                    .icon(IconName::Delete)
                    .label(match self.ed_truncate {
                        Tri::Follow => "清空目标: 跟随全局",
                        Tri::On => "清空目标: 强制清空(危险)",
                        Tri::Off => "清空目标: 不清空",
                    })
                    .tooltip("设置本表同步前是否 TRUNCATE 清空目标表")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.ed_truncate = this.ed_truncate.next();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("ed-create-tbl")
                    .xsmall()
                    .outline()
                    .label(match self.ed_create_table {
                        Tri::Follow => "自动建表: 跟随全局",
                        Tri::On => "自动建表: 自动创建",
                        Tri::Off => "自动建表: 禁止创建",
                    })
                    .tooltip("目标不存在时是否自动按源结构建表")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.ed_create_table = this.ed_create_table.next();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("ed-continue")
                    .xsmall()
                    .outline()
                    .label(match self.ed_continue_on_error {
                        Tri::Follow => "出错策略: 跟随全局",
                        Tri::On => "出错策略: 跳过继续",
                        Tri::Off => "出错策略: 立即中断",
                    })
                    .tooltip("单表同步出错时是否中断整个任务")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.ed_continue_on_error = this.ed_continue_on_error.next();
                        cx.notify();
                    })),
            );

        let opt_row_2 = h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("ed-strip-ai")
                    .xsmall()
                    .outline()
                    .label(match self.ed_strip_auto_increment {
                        Tri::Follow => "自增属性: 跟随全局",
                        Tri::On => "自增属性: 剥离自增",
                        Tri::Off => "自增属性: 保留自增",
                    })
                    .tooltip("建表时是否剥离 AUTO_INCREMENT")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.ed_strip_auto_increment = this.ed_strip_auto_increment.next();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("ed-strip-fk")
                    .xsmall()
                    .outline()
                    .label(match self.ed_strip_foreign_keys {
                        Tri::Follow => "外键约束: 跟随全局",
                        Tri::On => "外键约束: 剥离外键",
                        Tri::Off => "外键约束: 保留外键",
                    })
                    .tooltip("建表时是否剥离外键约束")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.ed_strip_foreign_keys = this.ed_strip_foreign_keys.next();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("ed-map-coll")
                    .xsmall()
                    .outline()
                    .label(match self.ed_map_collation {
                        Tri::Follow => "字符集降级: 跟随全局",
                        Tri::On => "字符集降级: 自动降级",
                        Tri::Off => "字符集降级: 保持原样",
                    })
                    .tooltip("跨版本建表时是否降级 collation")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.ed_map_collation = this.ed_map_collation.next();
                        cx.notify();
                    })),
            );

        block = block.child(
            v_flex()
                .gap_1()
                .child(field_label("单表高级覆盖选项", muted))
                .child(opt_row_1)
                .child(opt_row_2),
        );

        block.into_any_element()
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
            let target_display = if t.table == t.target_table {
                t.table.clone()
            } else {
                format!("{} → {}", t.table, t.target_table)
            };
            let (color, icon, text) = if t.syncable {
                (
                    success,
                    IconName::CircleCheck,
                    format!(
                        "{target_display} · 预计 {}{} 行",
                        if t.planned_rows_exact { "" } else { "≈" },
                        t.planned_rows
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
