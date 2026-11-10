//! 顶层骨架：自定义标题栏、状态栏、数据源页、日志页与设计系统小工具。

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::{scroll::ScrollableElement as _, Icon, IconName, WindowExt as _, *};
use gpui_kit::*;

use crate::app::RowBridgeApp;
use crate::model::*;

impl Render for RowBridgeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.smoke_done {
            self.smoke_done = true;
            self.maybe_run_smoke(window, cx);
            self.maybe_run_dst_smoke(window, cx);
        }
        let bg = cx.theme().background;
        let fg = cx.theme().foreground;
        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(self.title_bar(cx))
            .child(match self.view {
                Tab::Sync => self.sync_body(cx),
                Tab::Active => self.active_body(cx),
                Tab::Sources => self.sources_body(cx),
                Tab::Log => self.log_body(cx),
            })
    }
}

impl RowBridgeApp {
    fn title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().info;
        let accent_fg = cx.theme().background;
        let seg_bg = cx.theme().accent;

        let mut left = h_flex().items_center().gap_2();
        left = left
            .child(
                div()
                    .size(px(22.))
                    .rounded_md()
                    .bg(accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Network).xsmall().text_color(accent_fg)),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("RowBridge"),
            )
            .child(div().text_xs().text_color(muted).child("MySQL 数据同步"));

        if self.is_same_physical_db(cx) {
            left = left.child(chip("⚠️ 源与目标为同一库", cx.theme().danger, seg_bg));
        } else if self.is_same_instance(cx) {
            left = left.child(chip("同实例跨库", cx.theme().info, seg_bg));
        }

        if let Some((ok, msg)) = &self.status {
            let color = if *ok {
                cx.theme().success
            } else {
                cx.theme().danger
            };
            left = left.child(chip(msg.clone(), color, seg_bg));
        }

        let running_count = self.tasks.iter().filter(|t| t.is_active()).count();
        let active_label = if running_count > 0 {
            format!("进行中 ({running_count})")
        } else {
            "进行中".to_string()
        };

        let mut nav = h_flex().gap_1().p_1().rounded_md().bg(seg_bg);
        for (tab, label_str, icon) in [
            (Tab::Sync, "同步".to_string(), IconName::Replace),
            (Tab::Active, active_label, IconName::Play),
            (Tab::Sources, "数据源".to_string(), IconName::HardDrive),
            (Tab::Log, "日志".to_string(), IconName::SquareTerminal),
        ] {
            let button = Button::new(format!("nav-{:?}", tab))
                .xsmall()
                .icon(icon)
                .label(label_str);
            let button = if self.view == tab {
                button.primary()
            } else {
                button.ghost()
            };
            nav = nav.child(
                button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.view = tab;
                    cx.notify();
                })),
            );
        }

        TitleBar::new()
            .h(px(40.))
            .border_color(border)
            .bg(cx.theme().title_bar)
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .items_center()
                    .justify_between()
                    .pr_3()
                    .child(left)
                    .child(nav),
            )
            .into_any_element()
    }

    fn sync_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let bg = cx.theme().background;
        v_flex()
            .flex_1()
            .min_h_0()
            .bg(bg)
            .px_3()
            .pb_2()
            .gap_2()
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .child(self.side_pane(crate::model::DbSide::Src, cx))
                    .child(self.center_pane(cx))
                    .child(self.side_pane(crate::model::DbSide::Dst, cx)),
            )
            .child(self.status_bar(cx))
            .border_color(border)
            .into_any_element()
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;

        let mut bar = h_flex()
            .h(px(24.))
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_1()
            .text_xs()
            .text_color(muted)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::Frame).xsmall())
                    .child(format!(
                        "已选 {} 张 · 估算 {} 行",
                        self.selected_count(),
                        human_rows(self.estimated_rows())
                    )),
            );

        if !self.logs.is_empty() {
            bar = bar.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .child(self.logs[self.logs.len() - 1].message.clone()),
            );
        } else {
            bar = bar.child(div().flex_1());
        }

        let host = self.src.host_text(cx).trim().to_string();
        if !host.is_empty() {
            bar = bar.child(
                div()
                    .max_w(px(320.))
                    .truncate()
                    .text_xs()
                    .child(format!("{host}:{}", self.src.port_text(cx))),
            );
        }
        let _ = border;
        bar.into_any_element()
    }

    fn log_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().accent;

        let total_count = self.logs.len();
        let filtered_logs: Vec<_> = self
            .logs
            .iter()
            .filter(|l| match self.log_filter {
                Some(f) => l.level.as_ref() == f,
                None => true,
            })
            .collect();

        let mut head = card_head("运行日志", IconName::SquareTerminal, muted, border);
        head = head
            .child(chip(format!("{total_count} 条记录"), muted, accent))
            .child(div().flex_1());

        // 过滤按钮
        let mut filter_row = h_flex().gap_1();
        for (f, name) in [
            (None, "全部"),
            (Some("error"), "错误"),
            (Some("warn"), "警告"),
            (Some("info"), "信息"),
        ] {
            let active = self.log_filter == f;
            let btn = Button::new(format!("log-f-{name}")).xsmall().label(name);
            let btn = if active { btn.primary() } else { btn.ghost() };
            filter_row = filter_row.child(btn.on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.log_filter = f;
                    cx.notify();
                },
            )));
        }

        // 复制全部按钮
        let copy_btn = Button::new("copy-logs")
            .xsmall()
            .ghost()
            .icon(IconName::Copy)
            .label("复制全部")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                let lines: Vec<String> = this
                    .logs
                    .iter()
                    .map(|l| format!("[{}] [{}] {}", l.time, l.level.to_uppercase(), l.message))
                    .collect();
                let text = lines.join("\n");
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                this.push_log("info", "日志已复制到剪贴板");
                cx.notify();
            }));

        let clear_btn = Button::new("clear-logs")
            .xsmall()
            .ghost()
            .icon(IconName::Delete)
            .label("清空")
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.logs.clear();
                cx.notify();
            }));

        head = head.child(filter_row).child(copy_btn).child(clear_btn);

        let is_empty_filtered = filtered_logs.is_empty();
        let mut list = v_flex().gap_1().p_3();
        let display_logs: Vec<_> = filtered_logs.into_iter().rev().take(200).collect();
        for log in display_logs.into_iter().rev() {
            let color = match log.level.as_ref() {
                "error" => cx.theme().danger,
                "warn" => cx.theme().warning,
                _ => muted,
            };
            list = list.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .hover(|s| s.bg(accent))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_shrink_0()
                            .child(mono(log.time.clone(), muted)),
                    )
                    .child(div().w(px(46.)).flex_shrink_0().child(chip(
                        log.level.clone(),
                        color,
                        accent,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .child(log.message.clone()),
                    ),
            );
        }
        if self.logs.is_empty() {
            list = list.child(div().text_xs().text_color(muted).child("暂无日志。"));
        } else if is_empty_filtered {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("没有匹配当前筛选条件的日志。"),
            );
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .px_3()
            .pb_3()
            .child(
                panel_card(card_bg, border).flex_1().child(head).child(
                    div()
                        .id("log-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .child(list),
                ),
            )
            .into_any_element()
    }

    fn sources_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().accent;
        let path = rowbridge_core::connections::config_dir()
            .map(|p| p.join("connections.json").display().to_string())
            .unwrap_or_default();

        let mut list = v_flex().gap_2().p_3();

        if self.adding_source {
            list = list.child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(accent)
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("新建数据源"),
                            )
                            .child(
                                Button::new("close-new-src")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.adding_source = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(field_label("数据源名称", muted))
                                    .child(Input::new(&self.new_source_name).small()),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(field_label("主机", muted))
                                    .child(Input::new(&self.new_source_host).small()),
                            )
                            .child(
                                v_flex()
                                    .w(px(84.))
                                    .gap_1()
                                    .child(field_label("端口", muted))
                                    .child(Input::new(&self.new_source_port).small()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(field_label("用户名", muted))
                                    .child(Input::new(&self.new_source_user).small()),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .gap_1()
                                    .child(field_label("密码", muted))
                                    .child(Input::new(&self.new_source_password).small()),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .pt_1()
                            .child(
                                Checkbox::new("new-src-tls")
                                    .small()
                                    .checked(self.new_source_tls)
                                    .label("启用 TLS")
                                    .on_change(cx.listener(|this, checked, _, cx| {
                                        this.new_source_tls = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("cancel-new-src")
                                            .small()
                                            .ghost()
                                            .label("取消")
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, _, cx| {
                                                    this.adding_source = false;
                                                    cx.notify();
                                                },
                                            )),
                                    )
                                    .child(
                                        Button::new("save-new-src")
                                            .small()
                                            .primary()
                                            .icon(IconName::Check)
                                            .label("保存数据源")
                                            .on_click(cx.listener(
                                                |this, _: &ClickEvent, window, cx| {
                                                    this.save_new_source(window, cx);
                                                    cx.notify();
                                                },
                                            )),
                                    ),
                            ),
                    ),
            );
        }

        for conn in &self.saved {
            let id = conn.id.clone();
            let id_dst = conn.id.clone();
            let remove_id = conn.id.clone();
            let name = conn.name.clone();
            let host = format!("{}@{}:{}", conn.username, conn.host, conn.port);
            let last = conn.last_schema.clone().unwrap_or_else(|| "未记录".into());
            list = list.child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .size(px(28.))
                            .rounded_md()
                            .bg(cx.theme().accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::HardDrive).small().text_color(muted)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(conn.name.clone()),
                            )
                            .child(div().text_xs().text_color(muted).child(host))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("最近库：{last}")),
                            ),
                    )
                    .child(
                        Button::new(format!("use-src-{id}"))
                            .xsmall()
                            .outline()
                            .icon(IconName::ArrowRight)
                            .label("用作源库")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.pick_saved(crate::model::DbSide::Src, &id, window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(format!("use-dst-{id_dst}"))
                            .xsmall()
                            .outline()
                            .icon(IconName::ArrowDown)
                            .label("用作目标库")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.pick_saved(crate::model::DbSide::Dst, &id_dst, window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(format!("del-{remove_id}"))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Delete)
                            .tooltip("删除此数据源")
                            .on_click(cx.listener(move |_this, _: &ClickEvent, window, cx| {
                                let entity = cx.entity().clone();
                                let remove_id = remove_id.clone();
                                let name = name.clone();
                                window.open_alert_dialog(cx, move |alert, _w, _cx| {
                                    let entity = entity.clone();
                                    let remove_id = remove_id.clone();
                                    alert
                                        .confirm()
                                        .title(format!("删除数据源「{name}」"))
                                        .description("确定要删除这个数据源配置吗？此操作无法撤销。")
                                        .ok_variant(ButtonVariant::Danger)
                                        .ok_text("确认删除")
                                        .cancel_text("取消")
                                        .on_ok(move |_e, _w, cx| {
                                            entity.update(cx, |this, cx| {
                                                this.remove_saved(&remove_id);
                                                this.push_log("info", "已删除数据源");
                                                cx.notify();
                                            });
                                            true
                                        })
                                });
                            })),
                    ),
            );
        }

        if self.saved.is_empty() && !self.adding_source {
            list = list.child(div().text_xs().text_color(muted).child(
                "还没有保存的数据源：点击右上角「＋ 新建数据源」或在「同步」页填好后点「保存」。",
            ));
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .px_3()
            .pb_3()
            .child(
                panel_card(card_bg, border)
                    .flex_1()
                    .child(
                        card_head("已保存的数据源", IconName::HardDrive, muted, border)
                            .child(
                                Button::new("toggle-add-src")
                                    .xsmall()
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label(if self.adding_source {
                                        "收起表单"
                                    } else {
                                        "新建数据源"
                                    })
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.adding_source = !this.adding_source;
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(muted).child(path)),
                    )
                    .child(
                        div()
                            .id("sources-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .child(list),
                    ),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------ 进行中任务列表与详情

    fn active_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let card_bg = cx.theme().title_bar;
        let muted = cx.theme().muted_foreground;
        let accent = cx.theme().accent;
        let success = cx.theme().success;
        let warning = cx.theme().warning;
        let danger = cx.theme().danger;
        let fg = cx.theme().foreground;
        let list_active = cx.theme().list_active;
        let list_hover = cx.theme().list_hover;
        let primary = cx.theme().primary;

        // 左侧：任务列表
        let mut task_list = v_flex().gap_2().p_3();
        if self.tasks.is_empty() {
            task_list = task_list.child(
                v_flex()
                    .items_center()
                    .justify_center()
                    .py_8()
                    .gap_2()
                    .child(
                        Icon::new(IconName::LayoutDashboard)
                            .large()
                            .text_color(muted),
                    )
                    .child(label("暂无同步任务", muted))
                    .child(
                        Button::new("go-sync-tab")
                            .xsmall()
                            .outline()
                            .label("前往同步界面添加任务")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view = Tab::Sync;
                                cx.notify();
                            })),
                    ),
            );
        } else {
            for t in &self.tasks {
                let id = t.id.clone();
                let is_selected = self.selected_task_id.as_deref() == Some(&id);
                let (status_color, status_text) = match t.status {
                    TaskStatus::Running => (success, "运行中"),
                    TaskStatus::Paused => (warning, "已暂停"),
                    TaskStatus::Completed => (muted, "已完成"),
                    TaskStatus::Cancelled => (muted, "已取消"),
                    TaskStatus::Failed => (danger, "失败"),
                };

                let done = t.total_done();
                let planned = t.total_planned();
                let pct = if planned > 0 {
                    (done as f32 * 100.0 / planned as f32).clamp(0.0, 100.0)
                } else if t.status == TaskStatus::Completed {
                    100.0
                } else {
                    0.0
                };

                let mut card = v_flex()
                    .id(format!("task-card-{id}"))
                    .p_2p5()
                    .rounded_md()
                    .gap_1p5()
                    .cursor_pointer()
                    .border_1();

                card = if is_selected {
                    card.bg(list_active).border_color(primary)
                } else {
                    card.bg(accent)
                        .border_color(border)
                        .hover(move |s| s.bg(list_hover))
                };

                // 顶行：库名与状态胶囊
                let top_line = h_flex()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t.name.clone()),
                    )
                    .child(chip(status_text, status_color, card_bg));

                // 第二行：数据源 → 数据源
                let src_line = div()
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .child(t.routes());

                // 第三行：表摘要 + 时间
                let meta_line = h_flex()
                    .gap_2()
                    .items_center()
                    .text_xs()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(t.tables_summary()),
                    )
                    .child(mono(t.created_at.clone(), muted));

                // 进度条
                let progress_bar = div().h(px(4.)).w_full().rounded_full().bg(card_bg).child(
                    div()
                        .h_full()
                        .w(gpui::relative(pct / 100.0))
                        .rounded_full()
                        .bg(if t.status == TaskStatus::Running {
                            cx.theme().primary
                        } else {
                            status_color
                        }),
                );

                // 底部行：完成进度文字
                let bot_line = h_flex()
                    .justify_between()
                    .items_center()
                    .child(mono(
                        format!(
                            "{:.1}% · {}/{} 行",
                            pct,
                            human_rows(done),
                            human_rows(planned)
                        ),
                        muted,
                    ))
                    .child(label(
                        format!("{}/{} 表", t.done_tables_count(), t.tables.len()),
                        muted,
                    ));

                card = card
                    .child(top_line)
                    .child(src_line)
                    .child(meta_line)
                    .child(progress_bar)
                    .child(bot_line)
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, _, cx| {
                            this.selected_task_id = Some(id.clone());
                            cx.notify();
                        }
                    }));

                task_list = task_list.child(card);
            }
        }

        let left_panel = panel_card(card_bg, border)
            .w(px(340.))
            .h_full()
            .flex_shrink_0()
            .child(
                card_head("任务列表", IconName::LayoutDashboard, muted, border)
                    .child(div().flex_1())
                    .child(chip(format!("{} 个", self.tasks.len()), muted, accent)),
            )
            .child(
                div()
                    .id("tasks-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(task_list),
            );

        // 右侧：选中任务详情
        let selected_task = self
            .selected_task_id
            .as_ref()
            .and_then(|id| self.tasks.iter().find(|t| &t.id == id))
            .or_else(|| self.tasks.first());

        let right_panel = if let Some(task) = selected_task {
            let task_id = task.id.clone();
            let (status_color, status_text) = match task.status {
                TaskStatus::Running => (success, "运行中"),
                TaskStatus::Paused => (warning, "已暂停"),
                TaskStatus::Completed => (muted, "已完成"),
                TaskStatus::Cancelled => (muted, "已取消"),
                TaskStatus::Failed => (danger, "失败"),
            };

            let mut head = card_head(&task.name, IconName::Replace, muted, border);
            head = head.child(chip(status_text, status_color, accent));
            head = head.child(div().flex_1());

            // 控制操作按钮
            match task.status {
                TaskStatus::Running => {
                    let tid = task_id.clone();
                    head = head.child(
                        Button::new("task-pause")
                            .xsmall()
                            .outline()
                            .icon(IconName::Pause)
                            .label("暂停")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pause_task(&tid);
                                cx.notify();
                            })),
                    );
                    let tid2 = task_id.clone();
                    head = head.child(
                        Button::new("task-cancel")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .label("取消")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_task(&tid2);
                                cx.notify();
                            })),
                    );
                }
                TaskStatus::Paused => {
                    let tid = task_id.clone();
                    head = head.child(
                        Button::new("task-resume")
                            .xsmall()
                            .primary()
                            .icon(IconName::Play)
                            .label("继续")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.resume_task(&tid);
                                cx.notify();
                            })),
                    );
                    let tid2 = task_id.clone();
                    head = head.child(
                        Button::new("task-cancel-paused")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .label("取消")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_task(&tid2);
                                cx.notify();
                            })),
                    );
                }
                _ => {
                    let tid = task_id.clone();
                    head = head.child(
                        Button::new("task-remove")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Delete)
                            .label("删除记录")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_task(&tid);
                                cx.notify();
                            })),
                    );
                }
            }

            let done = task.total_done();
            let planned = task.total_planned();
            let pct = if planned > 0 {
                (done as f32 * 100.0 / planned as f32).clamp(0.0, 100.0)
            } else if task.status == TaskStatus::Completed {
                100.0
            } else {
                0.0
            };

            let mut body = v_flex().gap_3().p_3();

            // 1. 总体进度仪表盘卡片
            let stats_card = v_flex()
                .p_3()
                .rounded_md()
                .bg(accent)
                .gap_2()
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::BOLD)
                                .child(format!("{:.1}%", pct)),
                        )
                        .child(label(
                            format!(
                                "{}/{} 表已完成",
                                task.done_tables_count(),
                                task.tables.len()
                            ),
                            muted,
                        )),
                )
                .child(
                    div().h(px(6.)).w_full().rounded_full().bg(card_bg).child(
                        div()
                            .h_full()
                            .w(gpui::relative(pct / 100.0))
                            .rounded_full()
                            .bg(if task.status == TaskStatus::Running {
                                cx.theme().primary
                            } else {
                                status_color
                            }),
                    ),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .text_xs()
                        .child(mono(
                            format!("已同步 {} / 估算 {}", human_rows(done), human_rows(planned)),
                            muted,
                        ))
                        .child(label(
                            format!("源库: {} → 目标库: {}", task.src_schema, task.dst_schema),
                            muted,
                        )),
                )
                .child({
                    let src = format!("{} · {}", task.src_name, task.src_ep);
                    let dst = format!("{} · {}", task.dst_name, task.dst_ep);
                    h_flex()
                        .gap_1()
                        .items_center()
                        .text_xs()
                        .child(chip(src, muted, card_bg))
                        .child(Icon::new(IconName::ArrowRight).xsmall().text_color(muted))
                        .child(chip(dst, muted, card_bg))
                });
            body = body.child(stats_card);

            // 2. 表级子进度清单
            let mut table_rows = v_flex().gap_1();
            for tbl in &task.tables {
                let p = task
                    .progress
                    .get(tbl)
                    .cloned()
                    .unwrap_or_else(|| TableProgress::pending(0));
                let tbl_pct = p.percent();
                let tbl_status_badge = match p.status {
                    "running" => chip(format!("{:.0} 行/s", p.rps), cx.theme().primary, card_bg),
                    "done" => chip("已完成", success, card_bg),
                    "failed" => chip("失败", danger, card_bg),
                    _ => chip("排队中", muted, card_bg),
                };

                table_rows = table_rows.child(
                    v_flex()
                        .p_2()
                        .rounded_md()
                        .bg(accent)
                        .gap_1()
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(tbl.clone()),
                                )
                                .child(tbl_status_badge),
                        )
                        .child(
                            div().h(px(3.)).w_full().rounded_full().bg(card_bg).child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(tbl_pct / 100.0))
                                    .rounded_full()
                                    .bg(if p.status == "done" {
                                        success
                                    } else {
                                        cx.theme().primary
                                    }),
                            ),
                        )
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(mono(
                                    format!(
                                        "{} / {} 行",
                                        human_rows(p.rows),
                                        human_rows(p.planned)
                                    ),
                                    muted,
                                ))
                                .child(mono(
                                    if p.cursor.is_empty() {
                                        String::new()
                                    } else {
                                        format!("游标: {}", p.cursor)
                                    },
                                    muted,
                                )),
                        ),
                );
            }

            body = body.child(
                v_flex()
                    .gap_1p5()
                    .child(field_label("各表进度", muted))
                    .child(
                        div()
                            .id("table-prog-scroll")
                            .max_h(px(260.))
                            .overflow_y_scrollbar()
                            .child(table_rows),
                    ),
            );

            // 3. 任务日志
            let mut log_rows = v_flex().gap_0p5();
            for item in task.logs.iter().rev().take(40) {
                let col = match item.level.as_ref() {
                    "error" => danger,
                    "warn" => warning,
                    _ => muted,
                };
                log_rows = log_rows.child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .text_xs()
                        .child(mono(item.time.clone(), muted))
                        .child(chip(item.level.clone(), col, card_bg))
                        .child(div().flex_1().text_color(fg).child(item.message.clone())),
                );
            }

            body = body.child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1p5()
                    .child(field_label("任务日志", muted))
                    .child(
                        div()
                            .id("task-logs-scroll")
                            .flex_1()
                            .min_h(px(140.))
                            .overflow_y_scrollbar()
                            .p_2()
                            .rounded_md()
                            .bg(accent)
                            .child(log_rows),
                    ),
            );

            panel_card(card_bg, border)
                .flex_1()
                .h_full()
                .min_w_0()
                .child(head)
                .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(body))
        } else {
            panel_card(card_bg, border)
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .child(label("未选中任何任务", muted))
        };

        h_flex()
            .size_full()
            .gap_3()
            .p_3()
            .child(left_panel)
            .child(right_panel)
            .into_any_element()
    }
}

// ---------------------------------------------------------------- 设计系统小工具

/// 圆角卡片容器。
pub(crate) fn panel_card(bg: Hsla, border: Hsla) -> Div {
    v_flex()
        .min_h_0()
        .bg(bg)
        .rounded_lg()
        .border_1()
        .border_color(border)
        .overflow_hidden()
}

/// 卡片标题行。
pub(crate) fn card_head(
    title: impl Into<SharedString>,
    icon: IconName,
    muted: Hsla,
    border: Hsla,
) -> Div {
    h_flex()
        .h(px(38.))
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(border)
        .child(Icon::new(icon).small().text_color(muted))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
}

/// 状态小胶囊。
pub(crate) fn chip(text: impl Into<SharedString>, color: Hsla, bg: Hsla) -> AnyElement {
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .text_color(color)
        .text_xs()
        .child(text.into())
        .into_any_element()
}

pub(crate) fn label(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .text_xs()
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

pub(crate) fn field_label(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .text_xs()
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

pub(crate) fn divider(color: Hsla) -> AnyElement {
    div().h(px(1.)).w_full().bg(color).into_any_element()
}

/// 等宽数字，便于对齐行数 / 游标。
pub(crate) fn mono(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .text_xs()
        .font_family("SF Mono")
        .text_color(color)
        .child(text.into())
        .into_any_element()
}
