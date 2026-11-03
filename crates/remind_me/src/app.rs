use gpui_kit::{
    InteractiveElement, ParentElement, Render, Role, StatefulInteractiveElement, Styled, Window,
    assets::IconName,
    base::StyledExt,
    component::{
        Sizable, TitleBar,
        badge::Badge,
        button::{Button, ButtonVariants},
        menu::AppMenuBar,
    },
    div,
    prelude::{Context, IntoElement},
};

pub struct RemindMe;

impl RemindMe {
    pub fn new() -> Self {
        Self {}
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
