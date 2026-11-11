use auto_update::{AutoUpdateEvent, AutoUpdateStatus, AutoUpdater};
use gpui_kit::{
    Entity, InteractiveElement, ParentElement, Render, Role, StatefulInteractiveElement, Styled,
    Subscription, Window,
    assets::IconName,
    base::{StyledExt, h_flex},
    component::{
        ActiveTheme, Sizable, Theme, TitleBar,
        button::{Button, ButtonVariants},
        progress::ProgressCircle,
    },
    div,
    prelude::{Context, IntoElement},
    px,
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

    /// 当前状态对应的操作按钮，下载与安装都由用户点击触发
    fn update_actions(
        &self,
        cx: &Context<Self>,
        status: &AutoUpdateStatus,
    ) -> Option<impl IntoElement> {
        let theme = cx.theme();

        match status {
            AutoUpdateStatus::UpdateAvailable { .. } => {
                let updater = self.updater.clone();

                Some(
                    Button::new("download_update")
                        .child(div().child("发现新版本，点击下载").text_size(px(11.0)))
                        .cursor_pointer()
                        .mr_4()
                        .bg(theme.title_bar)
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| {
                            let _ = updater.update(cx, |updater, cx| updater.download_update(cx));
                        })
                        .into_any_element(),
                )
            }
            AutoUpdateStatus::Downloading { version } => {
                let updater = self.updater.clone();
                let percent = self
                    .updater
                    .read(cx)
                    .download_progress()
                    .and_then(|p| p.fraction());

                Some(
                    Button::new("update_status")
                        .cursor_pointer()
                        .child(
                            div()
                                .child(match percent {
                                    Some(fraction) => h_flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            ProgressCircle::new("update_download_progress")
                                                .value(fraction * 100.0)
                                                .small(),
                                        )
                                        .child(format!("{}%", (fraction * 100.0) as u8)),
                                    None => div().child(format!("正在下载 v{version}…")),
                                })
                                .text_size(px(11.0)),
                        )
                        .mr_4()
                        .bg(theme.title_bar)
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| {
                            updater.update(cx, |updater, cx| updater.cancel(cx));
                        })
                        .tooltip("点击取消下载")
                        .into_any_element(),
                )
            }
            AutoUpdateStatus::Downloaded { .. } => {
                let updater = self.updater.clone();

                Some(
                    Button::new("install_update")
                        .child(div().child("点击重启应用以更新").text_size(px(11.0)))
                        .cursor_pointer()
                        .mr_4()
                        .bg(theme.title_bar)
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| {
                            let installed =
                                updater.update(cx, |updater, cx| updater.install_update(cx));
                            if installed.is_ok() {
                                cx.quit();
                            }
                        })
                        .into_any_element(),
                )
            }
            AutoUpdateStatus::Failed { error } => {
                let updater = self.updater.clone();

                Some(
                    Button::new("update_failed")
                        .child(div().text_size(px(11.0)).child("检查更新失败"))
                        .mr_4()
                        .bg(theme.title_bar)
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| {
                            // 失败后点击按钮重置状态，主要是为了隐藏此按钮
                            updater.update(cx, |updater, cx| updater.reset(cx))
                        })
                        .tooltip(format!("{error}"))
                        .into_any_element(),
                )
            }
            AutoUpdateStatus::Idle
            | AutoUpdateStatus::Checking
            | AutoUpdateStatus::UpToDate { .. }
            | AutoUpdateStatus::Installing { .. } => None,
        }
    }
}

impl Render for RemindMe {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.updater.read(cx).status().clone();
        let update_btn = self.update_actions(cx, &status);

        div()
            .id("root_view")
            .role(Role::Window)
            .flex()
            .flex_col()
            .size_full()
            .child(
                TitleBar::new().child(
                    div()
                        .w_full()
                        .grid()
                        .grid_cols(3)
                        .items_center()
                        .child(
                            h_flex()
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
                        )
                        .child(
                            h_flex()
                                .justify_center()
                                .text_size(px(12.0))
                                .font_light()
                                .child("Remind Me"),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .mt_0p5()
                                .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .children(update_btn.into_iter()),
                        ),
                ),
            )
    }
}
