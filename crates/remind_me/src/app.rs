use gpui_kit::{
    InteractiveElement, ParentElement, Render, Role, StatefulInteractiveElement, Styled,
    Subscription, Window,
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
}

impl RemindMe {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 订阅系统明暗外观变化，同步主题。
        let _appearance_sub = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });

        Self { _appearance_sub }
    }
}

impl Render for RemindMe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().id("root_view").role(Role::Window).child(
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
    }
}
