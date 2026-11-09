mod app;
mod model;
mod ops;
mod views;
mod views_pane;

use gpui_kit::assets::Assets;
use gpui_kit::component::{TitleBar, *};
use gpui_kit::*;

fn main() {
    let app = gpui_kit::application().with_assets(Assets);
    app.run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(80.), px(80.)),
                size: size(px(1360.), px(860.)),
            })),
            window_min_size: Some(size(px(1080.), px(680.))),
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            Theme::change(ThemeMode::Dark, Some(window), cx);
            cx.new(|cx| app::RowBridgeApp::new(window, cx))
        })
        .expect("打开窗口失败");
    });
}
