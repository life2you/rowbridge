mod app;
mod model;
mod ops;
mod views;
mod views_pane;

use gpui_kit::assets::Assets;
use gpui_kit::component::{TitleBar, *};
use gpui_kit::*;

/// 从终端直接跑二进制时，Dock 里显示的是可执行文件的默认图标。
/// 这里把应用图标塞给 NSApplication，Dock / Cmd-Tab 就和正式 .app 一致了。
#[cfg(target_os = "macos")]
fn install_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    // 和 Tauri 版共用同一份图标资源，避免两套图各改一次。
    const ICON_PNG: &[u8] = include_bytes!("../../../src-tauri/icons/icon.png");

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(ICON_PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        let app = NSApplication::sharedApplication(mtm);
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}

fn main() {
    let app = gpui_kit::application().with_assets(Assets);
    app.run(move |cx| {
        // 必须在 GPUI 建好 AppKit 环境之后再改图标，否则会打乱它的 platform 初始化。
        #[cfg(target_os = "macos")]
        install_dock_icon();

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
