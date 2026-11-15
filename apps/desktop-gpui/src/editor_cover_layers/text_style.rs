use cap_project::{CoverLayer, CoverTextStyle};
use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use super::COVER_FONTS;
use crate::{
    editor_panels::FieldKey, editor_sidebar::ColorTarget, editor_window::EditorWindow, i18n::tr, ui,
};

pub(crate) struct CoverTextDialog {
    pub(crate) card: usize,
    pub(crate) layer: usize,
    draft: CoverLayer,
    pub(crate) focus: gpui::FocusHandle,
    tab_end: gpui::FocusHandle,
    return_focus: Option<gpui::FocusHandle>,
}

impl CoverTextDialog {
    pub(crate) fn apply_to(&self, item: &mut CoverLayer) {
        if item.id == self.draft.id
            && !item.locked
            && item.kind.text_style().is_some()
            && let Some(style) = self.draft.kind.text_style()
        {
            item.kind.set_text_style(style);
            item.font_size = self.draft.font_size;
            item.stroke_width = self.draft.stroke_width;
            item.band_opacity = self.draft.band_opacity;
        }
    }
}

impl EditorWindow {
    pub(super) fn open_cover_text_dialog(
        &mut self,
        card: usize,
        layer: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_canvas_text_edit(cx);
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        self.sidebar.menu = None;
        let Some(draft) = self
            .cover_layer(card, layer)
            .filter(|item| !item.locked && item.kind.text_style().is_some())
            .cloned()
        else {
            return;
        };
        self.show_cover_on_canvas(card, cx);
        let focus = cx.focus_handle();
        let return_focus = window.focused(cx);
        focus.focus(window, cx);
        self.sidebar.title_cards.layers.text_dialog = Some(CoverTextDialog {
            card,
            layer,
            draft,
            focus,
            tab_end: cx.focus_handle().tab_stop(true),
            return_focus,
        });
        cx.notify();
    }

    fn close_cover_text_dialog(
        &mut self,
        apply: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        self.sidebar.menu = None;
        let Some(dialog) = self.sidebar.title_cards.layers.text_dialog.take() else {
            return;
        };
        let return_focus = dialog.return_focus.clone();
        if apply {
            self.edit_cover_layer(
                dialog.card,
                dialog.layer,
                window,
                cx,
                move |items, layer| {
                    dialog.apply_to(&mut items[layer]);
                },
            );
        }
        self.publish_project();
        if let Some(focus) = return_focus {
            focus.focus(window, cx);
        } else {
            self.focus_root(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn cover_appearance(&self, card: usize, layer: usize) -> Option<&CoverLayer> {
        self.sidebar
            .title_cards
            .layers
            .text_dialog
            .as_ref()
            .filter(|dialog| (dialog.card, dialog.layer) == (card, layer))
            .map(|dialog| &dialog.draft)
    }

    pub(crate) fn edit_cover_appearance(
        &mut self,
        card: usize,
        layer: usize,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut CoverLayer, &mut CoverTextStyle),
    ) {
        if let Some(dialog) = self.sidebar.title_cards.layers.text_dialog.as_mut()
            && (dialog.card, dialog.layer) == (card, layer)
            && let Some(mut style) = dialog.draft.kind.text_style()
        {
            change(&mut dialog.draft, &mut style);
            dialog.draft.kind.set_text_style(style);
            self.publish_project();
            cx.notify();
        }
    }

    pub(crate) fn cover_text_dialog_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.sidebar.title_cards.layers.text_dialog.is_none() {
            return false;
        }
        if let Some(direction) = ui::keyboard_nav::direction(&event.keystroke)
            && let Some(dialog) = &self.sidebar.title_cards.layers.text_dialog
        {
            window.prevent_default();
            self.sidebar.menu = None;
            match direction {
                ui::keyboard_nav::Direction::Next => {
                    if !dialog.focus.contains_focused(window, cx) {
                        dialog.focus.focus(window, cx);
                    }
                    window.focus_next(cx);
                    if dialog.tab_end.is_focused(window)
                        || !dialog.focus.contains_focused(window, cx)
                    {
                        dialog.focus.focus(window, cx);
                        window.focus_next(cx);
                    }
                }
                ui::keyboard_nav::Direction::Previous => {
                    if dialog.focus.is_focused(window) || !dialog.focus.contains_focused(window, cx)
                    {
                        dialog.tab_end.focus(window, cx);
                    }
                    window.focus_prev(cx);
                    if !dialog.focus.contains_focused(window, cx) {
                        dialog.tab_end.focus(window, cx);
                        window.focus_prev(cx);
                    }
                }
            }
            cx.notify();
            return true;
        }
        let key = event.keystroke.key.as_str();
        if !event.is_held {
            if self.sidebar.menu.is_some() && self.sidebar_menu_key(key, window, cx) {
                return true;
            }
            if key == "escape" {
                if self.sidebar.color_picker.is_some() {
                    self.close_color_picker(cx);
                } else {
                    self.close_cover_text_dialog(false, window, cx);
                }
            }
        }
        true
    }

    pub(crate) fn render_cover_text_dialog(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.sidebar.title_cards.layers.text_dialog.as_ref()?;
        let (card, layer) = (dialog.card, dialog.layer);
        let style = dialog.draft.kind.text_style()?;
        let theme = self.theme;
        let controls = |label, child: AnyElement| {
            div()
                .flex_1()
                .min_w_0()
                .child(ui::Field::stacked(&theme, label).child(child))
        };
        let font = style.font.as_deref().unwrap_or(COVER_FONTS[0]);
        let number = |key, unit| self.render_number_field(key, unit, 76.);
        let toggle =
            ui::Toggle::plain(&theme, "cover-text-background-toggle", style.band.is_some())
                .label(tr::editor::cover_controls::background())
                .focusable()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.edit_cover_appearance(card, layer, cx, |item, style| {
                        if style.band.take().is_none() {
                            style.band = Some("#111111".into());
                            if item.band_opacity.is_none() {
                                item.band_opacity = Some(0.85);
                            }
                        }
                    });
                }));
        let height = (f32::from(window.viewport_size().height) - 32.).max(180.);
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_end()
                .p(px(16.))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(gpui::hsla(0., 0., 0., 0.24))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .id("cover-text-dialog")
                        .role(gpui::Role::Dialog)
                        .aria_label(tr::editor::cover_controls::custom())
                        .track_focus(&dialog.focus)
                        .tab_group()
                        .tab_stop(false)
                        .occlude()
                        .w(px(390.))
                        .max_w_full()
                        .max_h(px(height))
                        .flex()
                        .flex_col()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(Hsla::from(theme.editor.line))
                        .bg(Hsla::from(theme.editor.card))
                        .text_color(Hsla::from(theme.editor.text_1))
                        .shadow(theme.editor.pop_shadow())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex_none()
                                .px(px(16.))
                                .py(px(12.))
                                .border_b_1()
                                .border_color(Hsla::from(theme.editor.line))
                                .text_size(px(15.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(tr::editor::cover_controls::custom()),
                        )
                        .child(
                            div()
                                .id("cover-text-dialog-fields")
                                .min_h_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap(px(14.))
                                .p(px(16.))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(12.))
                                        .child(controls(
                                            tr::editor::cover_controls::font(),
                                            self.menu_select_owned(
                                                crate::editor_tabs::SidebarMenu::CoverFontFamily(
                                                    card, layer,
                                                ),
                                                "cover-custom-font".into(),
                                                crate::editor_panels::font_family_label(font)
                                                    .into(),
                                                cx,
                                            )
                                            .into_any_element(),
                                        ))
                                        .child(controls(
                                            tr::editor::cover_controls::font_size(),
                                            number(FieldKey::CoverTextSize(card, layer), "px")
                                                .into_any_element(),
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(12.))
                                        .child(controls(
                                            tr::editor::cover_controls::text_color(),
                                            self.render_color_input(
                                                ColorTarget::CoverText(card, layer),
                                                &style.color,
                                                cx,
                                            ),
                                        ))
                                        .child(controls(
                                            tr::editor::cover_controls::stroke_width(),
                                            number(FieldKey::CoverStrokeWidth(card, layer), "px")
                                                .into_any_element(),
                                        )),
                                )
                                .child(controls(
                                    tr::editor::cover_controls::stroke_color(),
                                    self.render_color_input(
                                        ColorTarget::CoverStroke(card, layer),
                                        style.stroke.as_deref().unwrap_or("#111111"),
                                        cx,
                                    ),
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .border_t_1()
                                        .border_color(Hsla::from(theme.editor.line))
                                        .pt(px(12.))
                                        .text_size(px(13.))
                                        .child(tr::editor::cover_controls::background())
                                        .child(toggle),
                                )
                                .children(style.band.as_deref().map(|band| {
                                    div()
                                        .flex()
                                        .gap(px(12.))
                                        .child(controls(
                                            tr::editor::cover_controls::background_color(),
                                            self.render_color_input(
                                                ColorTarget::CoverBackground(card, layer),
                                                band,
                                                cx,
                                            ),
                                        ))
                                        .child(controls(
                                            tr::editor::cover_controls::opacity(),
                                            number(
                                                FieldKey::CoverBackgroundOpacity(card, layer),
                                                "%",
                                            )
                                            .into_any_element(),
                                        ))
                                })),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .px(px(16.))
                                .py(px(12.))
                                .border_t_1()
                                .border_color(Hsla::from(theme.editor.line))
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "cover-style-cancel",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Md,
                                    )
                                    .label(tr::common::cancel())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.close_cover_text_dialog(false, window, cx)
                                        },
                                    )),
                                )
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "cover-style-apply",
                                        ui::ButtonVariant::Primary,
                                        ui::ButtonSize::Md,
                                    )
                                    .label(tr::editor::cover_controls::apply())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.close_cover_text_dialog(true, window, cx)
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .id("cover-text-tab-end")
                                .track_focus(&dialog.tab_end)
                                .tab_index(0)
                                .tab_stop(true)
                                .size(px(0.)),
                        ),
                )
                .into_any_element(),
        )
    }
}
