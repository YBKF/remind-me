#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod constants;

use auto_update::{AutoUpdateConfig, AutoUpdater, InstallerConfig, semver::Version};
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
    constants::{UPDATE_HELPER, UPDATE_MANIFEST_URL, UPDATE_PUBLIC_KEY},
};

rust_i18n::i18n!("../../locales", fallback = "en");

/// 安装策略：退出时由助手静默安装，安装成功后删除安装包并重启程序
///
/// 助手参数与安装包参数用 `--` 分隔，`launch` 追加的 `/CURRENTUSER` 或
/// `/ALLUSERS` 落在 `--` 之后，由助手原样转交给安装包
fn installer_config() -> InstallerConfig {
    InstallerConfig::builder()
        .program(UPDATE_HELPER)
        .args(bon::vec![
            "--installer-path",
            "{installer}",
            "--relaunch",
            "{app}",
            "--wait-pid",
            "{pid}",
            "--auto-remove",
            "--",
            "/SILENT",
        ])
        .per_user_args(bon::vec!["/CURRENTUSER"])
        .per_machine_args(bon::vec!["/ALLUSERS"])
        // 助手位于安装目录内，安装会覆盖它，先复制到临时目录再运行
        .relocate_to(std::env::temp_dir().join("RemindMe-update-helper"))
        .build()
}

/// 初始化 auto_update，启动时自动检查更新，下载与安装都由用户点击触发
fn init_auto_update(cx: &mut App) {
    let current_version =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION 不是合法 semver");
    let public_key = (!UPDATE_PUBLIC_KEY.is_empty()).then(|| UPDATE_PUBLIC_KEY.to_owned());

    let config = AutoUpdateConfig::builder()
        .current_version(current_version)
        .manifest_url(UPDATE_MANIFEST_URL)
        .maybe_public_key(public_key)
        .installer_config(installer_config())
        .build();

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
