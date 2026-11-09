//! 顶层骨架：自定义标题栏、状态栏、数据源页、日志页与设计系统小工具。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    scroll::ScrollableElement as _, Icon, IconName, TitleBar, *,
};
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
            .child(self.title_bar(cx))
            .child(match self.view {
                Tab::Sync => self.sync_body(cx),
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
        let primary = cx.theme().primary;
        let primary_fg = cx.theme().primary_foreground;
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
                    .child(
                        Icon::new(IconName::Network)
                            .xsmall()
                            .text_color(accent_fg),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("RowBridge"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("MySQL 数据同步"),
            );

        if self.running {
            left = left.child(chip("同步中", primary_fg, primary));
        }
        if let Some((ok, msg)) = &self.status {
            let color = if *ok {
                cx.theme().success
            } else {
                cx.theme().danger
            };
            left = left.child(chip(msg.clone(), color, seg_bg));
        }

        let mut nav = h_flex().gap_1().p_1().rounded_md().bg(seg_bg);
        for (tab, label, icon) in [
            (Tab::Sync, "同步", IconName::Replace),
            (Tab::Sources, "数据源", IconName::HardDrive),
            (Tab::Log, "日志", IconName::SquareTerminal),
        ] {
            let button = Button::new(format!("nav-{label}"))
                .xsmall()
                .icon(icon)
                .label(label);
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

        TitleBar::new()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
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
        let total_done: u64 = self.progress.values().map(|p| p.rows).sum();
        let planned: u64 = self.progress.values().map(|p| p.planned).sum();

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

        if !self.progress.is_empty() {
            bar = bar.child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(Icon::new(IconName::ChartPie).xsmall())
                    .child(format!("{} / {} 行", total_done, planned)),
            );
        }
        if !self.logs.is_empty() {
            bar = bar.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .child(self.logs[self.logs.len() - 1].1.clone()),
            );
        } else {
            bar = bar.child(div().flex_1());
        }

        let host = self
            .src
            .host_text(cx)
            .trim()
            .to_string();
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
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(color)
                            .child(level.clone()),
                    )
                    .child(div().flex_1().min_w_0().text_xs().child(msg.clone())),
            );
        }
        if self.logs.is_empty() {
            list = list.child(div().text_xs().text_color(muted).child("暂无日志。"));
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .px_3()
            .pb_3()
            .child(
                panel_card(card_bg, border)
                    .flex_1()
                    .child(card_head(
                        "运行日志",
                        IconName::SquareTerminal,
                        muted,
                        border,
                    ))
                    .child(
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
        let path = rowbridge_core::connections::config_dir()
            .map(|p| p.join("connections.json").display().to_string())
            .unwrap_or_default();

        let mut list = v_flex().gap_2().p_3();
        for conn in &self.saved {
            let id = conn.id.clone();
            let remove_id = conn.id.clone();
            let host = format!(
                "{}@{}:{}",
                conn.username, conn.host, conn.port
            );
            let last = conn
                .last_schema
                .clone()
                .unwrap_or_else(|| "未记录".into());
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
                        Button::new(format!("use-{id}"))
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
                        Button::new(format!("del-{remove_id}"))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Delete)
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
                    .child("还没有保存的数据源：在「同步」页填好连接后点「保存」。"),
            );
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
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(path),
                            ),
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
