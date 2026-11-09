#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod constants;

use auto_update::{AutoUpdateConfig, AutoUpdater, InstallerConfig};
use gpui_kit::{
    App, AppContext, WindowBounds, WindowOptions,
    component::{Theme, TitleBar},
    px, size,
};
use log::error;
use rust_i18n::t;
use utils::locale::detect_locale;

use crate::{
    app::RemindMe,
    constants::{UPDATE_MANIFEST_URL, UPDATE_PUBLIC_KEY},
};

rust_i18n::i18n!("../../locales", fallback = "en");

/// 初始化自动更新。必须在打开窗口之前调用。
fn init_auto_update(cx: &mut App) {
    let mut config = AutoUpdateConfig::parse(env!("CARGO_PKG_VERSION"), UPDATE_MANIFEST_URL)
        .expect("CARGO_PKG_VERSION 是合法 semver")
        .with_installer(InstallerConfig::inno_setup());

    if !UPDATE_PUBLIC_KEY.is_empty() {
        config = config.with_public_key(UPDATE_PUBLIC_KEY);
    }

    AutoUpdater::init(cx, config);
}

fn main() {
    tracing_subscriber::fmt::init();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            use gpui_kit::component as gpui_component;
            rust_i18n::extend!(gpui_component);

            // 系统语言 → rust-i18n 的当前语言。匹配不到时沿用当前默认语言，
            let fallback = rust_i18n::locale().to_string();
            let available: Vec<String> = rust_i18n::available_locales!()
                .iter()
                .map(|locale| locale.to_string())
                .collect();
            rust_i18n::set_locale(&detect_locale(&available, &fallback));

            gpui_kit::init(cx);
            init_auto_update(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(960.), px(540.)), cx)),
                window_min_size: Some(size(px(780.), px(440.))),
                titlebar: Some(TitleBar::title_bar_options()),
                ..Default::default()
            };

            if let Err(e) = gpui_kit::open_window(options, cx, |window, cx| {
                let app_name = t!("app.name");
                window.set_window_title(&app_name);
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| RemindMe::new(window, cx))
            }) {
                error!("failed to open window: {:?}", e);
                cx.quit();
            }
        });
}
