use auto_update::{AutoUpdateEvent, AutoUpdateStatus, AutoUpdater};
use gpui_kit::{
    App, Entity, InteractiveElement, ParentElement, Render, Role, StatefulInteractiveElement,
    Styled, Subscription, Window,
    assets::IconName,
    component::{
        Sizable, Theme, TitleBar,
        button::{Button, ButtonVariants},
    },
    div,
    prelude::{Context, IntoElement},
};

pub struct RemindMe {
    /// 系统明暗外观变化时同步主题。持有订阅，丢弃即取消。
    _appearance_sub: Subscription,
    /// 监听更新状态变化，驱动重绘。
    _update_sub: Subscription,
    updater: Entity<AutoUpdater>,
}

impl RemindMe {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 订阅系统明暗外观变化，同步主题。
        let _appearance_sub = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });

        // 更新器在 main 里通过 `AutoUpdater::init` 注册为全局单例。
        let updater = AutoUpdater::global(cx).expect("init_auto_update 必须在打开窗口前调用");
        let _update_sub = cx.subscribe(
            &updater,
            |_, _: Entity<AutoUpdater>, _: &AutoUpdateEvent, cx| cx.notify(),
        );

        Self {
            _appearance_sub,
            _update_sub,
            updater,
        }
    }

    fn update_label(updater: &Entity<AutoUpdater>, cx: &App) -> String {
        let updater = updater.read(cx);
        match updater.status() {
            AutoUpdateStatus::Idle | AutoUpdateStatus::UpToDate { .. } => String::new(),
            AutoUpdateStatus::Checking => "正在检查更新…".to_string(),
            AutoUpdateStatus::UpdateAvailable { version } => {
                format!("发现新版本 v{version}")
            }
            AutoUpdateStatus::Downloading { version } => {
                match updater.download_progress().and_then(|p| p.fraction()) {
                    Some(fraction) => {
                        format!("正在下载 v{version} ({:.0}%)", fraction * 100.0)
                    }
                    None => format!("正在下载 v{version}…"),
                }
            }
            AutoUpdateStatus::Downloaded { version } => {
                format!("v{version} 已就绪，退出应用时安装")
            }
            AutoUpdateStatus::Installing { version } => format!("正在安装 v{version}"),
            AutoUpdateStatus::Failed { error } => format!("更新失败：{error}"),
        }
    }
}

impl Render for RemindMe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let update_label = Self::update_label(&self.updater, cx);

        div()
            .id("root_view")
            .role(Role::Window)
            .flex()
            .flex_col()
            .size_full()
            .child(
                TitleBar::new().child(
                    div()
                        .flex()
                        .items_center()
                        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            Button::new("app_menu_btn")
                                .ghost()
                                .icon(IconName::Menu)
                                .small()
                                .cursor_pointer(),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .child(update_label),
            )
    }
}
