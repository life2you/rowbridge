//! 顶层布局与通用渲染小工具。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{scroll::ScrollableElement as _, *};
use gpui_kit::*;

use crate::app::RowBridgeApp;
use crate::model::{human_rows, Tab};

impl Render for RowBridgeApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.smoke_done {
            self.smoke_done = true;
            self.maybe_run_smoke(window, cx);
        }
        let bg = cx.theme().background;
        let fg = cx.theme().foreground;
        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .child(self.header(cx))
            .child(match self.view {
                Tab::Sync => self.sync_body(cx),
                Tab::Sources => self.sources_body(cx),
                Tab::Log => self.log_body(cx),
            })
    }
}

impl RowBridgeApp {
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;

        let mut nav = h_flex().gap_1().items_center();
        for (tab, label, id) in [
            (Tab::Sync, "同步", "nav-sync"),
            (Tab::Sources, "数据源", "nav-sources"),
            (Tab::Log, "日志", "nav-log"),
        ] {
            let button = Button::new(id).small().label(label);
            let button = if self.view == tab {
                button.primary()
            } else {
                button.ghost()
            };
            nav = nav.child(button.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.view = tab;
                cx.notify();
            })));
        }

        let mut left = h_flex().items_center().gap_2();
        left = left
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::BOLD)
                    .child("RowBridge"),
            )
            .child(badge("MySQL 数据同步", muted, cx.theme().muted));

        if self.running {
            left = left.child(badge("同步中", cx.theme().primary_foreground, cx.theme().primary));
        }
        if let Some((ok, msg)) = &self.status {
            let (color, background) = if *ok {
                (cx.theme().success, cx.theme().muted)
            } else {
                (cx.theme().danger, cx.theme().muted)
            };
            left = left.child(badge(msg.clone(), color, background));
        }

        h_flex()
            .h(px(44.))
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .px_3()
            .border_b_1()
            .border_color(border)
            .child(left)
            .child(nav)
            .into_any_element()
    }

    fn sync_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.side_pane(crate::model::DbSide::Src, cx))
                    .child(self.center_pane(cx))
                    .child(self.side_pane(crate::model::DbSide::Dst, cx)),
            )
            .child(self.status_bar(cx))
            .border_color(border)
            .into_any_element()
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let total_done: u64 = self.progress.values().map(|p| p.rows).sum();
        let planned: u64 = self.progress.values().map(|p| p.planned).sum();

        let mut bar = h_flex()
            .h(px(32.))
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .px_3()
            .border_t_1()
            .border_color(border);

        bar = bar.child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!(
                    "已选 {} 张 · 估算 {} 行",
                    self.selected_count(),
                    human_rows(self.estimated_rows())
                )),
        );

        if !self.progress.is_empty() {
            bar = bar.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("进度 {} / {} 行", total_done, planned)),
            );
        }
        if !self.logs.is_empty() {
            let last = self.logs[self.logs.len() - 1].1.clone();
            bar = bar.child(div().flex_1().min_w_0().text_xs().text_color(muted).child(last));
        }

        if self.running {
            bar = bar.child(
                Button::new("cancel-run")
                    .small()
                    .danger()
                    .label("取消")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.do_cancel();
                        cx.notify();
                    })),
            );
        } else if self.can_preflight() {
            bar = bar.child(
                Button::new("preflight-bar")
                    .small()
                    .label("预检")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.do_preflight(cx);
                        cx.notify();
                    })),
            );
        }

        bar.into_any_element()
    }

    fn log_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let mut list = v_flex().gap_1().p_3();
        for (level, msg) in &self.logs {
            let color = match level.as_ref() {
                "error" => cx.theme().danger,
                "warn" => cx.theme().warning,
                _ => muted,
            };
            list = list.child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().w(px(48.)).flex_shrink_0().text_xs().text_color(color).child(level.clone()))
                    .child(div().flex_1().min_w_0().text_xs().child(msg.clone())),
            );
        }
        if self.logs.is_empty() {
            list = list.child(div().text_xs().text_color(muted).child("暂无日志"));
        }

        div()
            .id("log-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .border_color(border)
            .child(list)
            .into_any_element()
    }

    fn sources_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let path = rowbridge_core::connections::config_dir()
            .map(|p| p.join("connections.json").display().to_string())
            .unwrap_or_default();

        let mut list = v_flex().gap_2().p_4();
        list = list
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::BOLD)
                    .child("已保存的数据源"),
            )
            .child(div().text_xs().text_color(muted).child(format!("配置文件：{path}")));

        for conn in &self.saved {
            let id = conn.id.clone();
            let remove_id = conn.id.clone();
            let summary = format!(
                "{}@{}:{}  ·  {}",
                conn.username, conn.host, conn.port, conn.name
            );
            list = list.child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(div().flex_1().min_w_0().text_sm().child(summary))
                    .child(
                        Button::new(format!("use-{id}"))
                            .small()
                            .label("用作源库")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.pick_saved(crate::model::DbSide::Src, &id, window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(format!("del-{remove_id}"))
                            .small()
                            .danger()
                            .label("删除")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.remove_saved(&remove_id);
                                cx.notify();
                            })),
                    ),
            );
        }

        if self.saved.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("还没有保存的数据源：在「同步」页填好连接后点「保存源库」。"),
            );
        }

        div()
            .id("sources-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(list)
            .into_any_element()
    }
}

// ---------------------------------------------------------------- 小工具

pub(crate) fn badge(
    text: impl Into<SharedString>,
    color: Hsla,
    background: Hsla,
) -> AnyElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(background)
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

pub(crate) fn section_title(text: impl Into<SharedString>, color: Hsla) -> AnyElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into())
        .into_any_element()
}
