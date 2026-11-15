use std::{cell::Cell, path::PathBuf, rc::Rc};

use cap_project::CoverLayerKind;
use gpui::{
    AnyElement, Bounds, Context, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, StatefulInteractiveElement, Styled,
    StyledImage, Window, div, img, prelude::FluentBuilder, px,
};

use crate::{
    editor_canvas::CanvasSelection,
    editor_crop::{self, CropBounds, CropDrag, CropHit, CropState, HANDLES, ResizeSession, Vec2},
    editor_window::EditorWindow,
    i18n::tr,
    ui,
};

const FULL_IMAGE: [f32; 4] = [0., 0., 1., 1.];

pub(crate) struct CoverCrop {
    card: usize,
    layer: usize,
    id: String,
    path: PathBuf,
    source: (u32, u32),
    state: CropState,
    area: Rc<Cell<Option<Bounds<Pixels>>>>,
}

fn source_bounds([x, y, width, height]: [f32; 4], source: (u32, u32)) -> CropBounds {
    let (w, h) = (f64::from(source.0), f64::from(source.1));
    CropBounds::new(
        f64::from(x) * w,
        f64::from(y) * h,
        f64::from(width) * w,
        f64::from(height) * h,
    )
}

fn normalized_bounds(bounds: CropBounds, source: (u32, u32)) -> [f32; 4] {
    let (w, h) = (f64::from(source.0), f64::from(source.1));
    let x = (bounds.x / w).clamp(0., 1.);
    let y = (bounds.y / h).clamp(0., 1.);
    [
        x as f32,
        y as f32,
        (bounds.width / w).clamp(0., 1. - x) as f32,
        (bounds.height / h).clamp(0., 1. - y) as f32,
    ]
}

impl CoverCrop {
    fn reset(&mut self) {
        let size = self.state.box_size;
        self.state = CropState::new(self.source, size, source_bounds(FULL_IMAGE, self.source));
        self.state.set_container(size, size);
    }

    fn down(&mut self, event: &MouseDownEvent) {
        let point = editor_crop::crop_local(self.area.get(), event.position);
        let state = &mut self.state;
        state.drag = Some(match editor_crop::hit_test(point, state.raw) {
            CropHit::Handle(handle) => CropDrag::Handle(ResizeSession {
                start_bounds: state.raw,
                is_alt: event.modifiers.alt,
                active_handle: handle,
                original_handle: handle,
            }),
            CropHit::Move => CropDrag::Region {
                start_offset: Vec2::new(point.x - state.raw.x, point.y - state.raw.y),
                bounds: state.raw,
            },
            CropHit::Draw => CropDrag::Overlay {
                restore: state.raw,
                session: ResizeSession {
                    start_bounds: CropBounds::new(point.x, point.y, 1., 1.),
                    is_alt: event.modifiers.alt,
                    active_handle: editor_crop::SE_HANDLE,
                    original_handle: editor_crop::SE_HANDLE,
                },
            },
        });
    }

    fn move_pointer(&mut self, event: &MouseMoveEvent) {
        let point = editor_crop::crop_local(self.area.get(), event.position);
        match self.state.drag {
            Some(CropDrag::Region {
                start_offset,
                bounds,
            }) => {
                let container = self.state.container_vec();
                let x = (point.x - start_offset.x)
                    .max(0.)
                    .min(container.x - bounds.width);
                let y = (point.y - start_offset.y)
                    .max(0.)
                    .min(container.y - bounds.height);
                self.state
                    .set_raw(editor_crop::move_bounds(bounds, Some(x), Some(y)));
            }
            Some(CropDrag::Handle(_) | CropDrag::Overlay { .. }) => {
                self.state
                    .resize_move(point, event.modifiers.alt, event.modifiers.shift, false);
            }
            None => {}
        }
    }

    fn release(&mut self) {
        if let Some(CropDrag::Overlay { restore, .. }) = self.state.drag.take()
            && (self.state.raw.width < 5. || self.state.raw.height < 5.)
        {
            self.state.set_raw(restore);
        }
    }
}

impl EditorWindow {
    pub(crate) fn open_cover_image_crop(
        &mut self,
        element: CanvasSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let CanvasSelection::CoverLayer(card, layer) = element else {
            return;
        };
        let Some(item) = self.cover_layer(card, layer).filter(|item| !item.locked) else {
            return;
        };
        let CoverLayerKind::Image { path, .. } = &item.kind else {
            return;
        };
        let path = self.project_path.join(path);
        let Ok(source) = image::image_dimensions(&path) else {
            return;
        };
        if source.0 == 0 || source.1 == 0 {
            return;
        }
        let viewport = window.viewport_size();
        let scale = (f32::from(viewport.width) * 0.72 / source.0 as f32)
            .min(f32::from(viewport.height) * 0.64 / source.1 as f32);
        let size = (source.0 as f32 * scale, source.1 as f32 * scale);
        if size.0 < 2. || size.1 < 2. {
            return;
        }
        let id = item.id.clone();
        let initial = source_bounds(item.image_crop.unwrap_or(FULL_IMAGE), source);
        let mut state = CropState::new(source, size, initial);
        state.set_container(size, size);
        self.stop_canvas_text_edit(cx);
        self.end_field_edit(cx);
        self.sidebar.title_cards.layers.crop = Some(CoverCrop {
            card,
            layer,
            id,
            path,
            source,
            state,
            area: Rc::default(),
        });
        self.focus_root(window, cx);
        cx.notify();
    }

    fn close_cover_image_crop(&mut self, apply: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(crop) = self.sidebar.title_cards.layers.crop.take() else {
            return;
        };
        if apply {
            let bounds = normalized_bounds(crop.state.real(), crop.source);
            let next = (bounds != FULL_IMAGE).then_some(bounds);
            self.edit_cover_layer(crop.card, crop.layer, window, cx, move |layers, index| {
                let item = &mut layers[index];
                if item.id != crop.id || item.locked || item.image_crop == next {
                    return;
                }
                let previous = item.image_crop.unwrap_or(FULL_IMAGE);
                item.pose.size.x *= f64::from(bounds[2] / previous[2]);
                item.pose.size.y *= f64::from(bounds[3] / previous[3]);
                item.image_crop = next;
            });
        }
        self.focus_root(window, cx);
        cx.notify();
    }

    pub(crate) fn cover_crop_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.sidebar.title_cards.layers.crop.is_some() {
            match key {
                "escape" => self.close_cover_image_crop(false, window, cx),
                "enter" => self.close_cover_image_crop(true, window, cx),
                _ => {}
            }
            return true;
        }
        if key == "escape" && self.sidebar.title_cards.layers.menu.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    pub(crate) fn render_cover_image_crop(
        &self,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let crop = self.sidebar.title_cards.layers.crop.as_ref()?;
        let theme = self.theme;
        let bounds = crop.state.raw;
        let (width, height) = crop.state.box_size;
        let area = crop.area.clone();
        let footprint = div()
            .id("cover-crop-image")
            .relative()
            .w(px(width))
            .h(px(height))
            .cursor_crosshair()
            .child(
                img(crop.path.clone())
                    .size_full()
                    .object_fit(gpui::ObjectFit::Fill),
            )
            .child(
                gpui::canvas(move |bounds, _, _| area.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .children(editor_crop::occluders(bounds, width, height))
            .child(editor_crop::thirds_grid(bounds))
            .children(
                HANDLES
                    .iter()
                    .copied()
                    .flat_map(|handle| editor_crop::corner_glyph(handle, bounds, false)),
            )
            .children(
                HANDLES
                    .iter()
                    .copied()
                    .map(|handle| editor_crop::handle_zone(handle, bounds, None)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| {
                    if let Some(crop) = this.sidebar.title_cards.layers.crop.as_mut() {
                        crop.down(event);
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            );
        let button = |id: &'static str, label: &'static str| {
            ui::EditorButton::plain(&theme, id)
                .outlined(&theme)
                .label(label)
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(gpui::hsla(0., 0., 0., 0.75)),
                )
                .child(
                    div()
                        .occlude()
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .p(px(16.))
                        .rounded(px(14.))
                        .bg(Hsla::from(theme.editor.card))
                        .text_color(Hsla::from(theme.editor.text_1))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(tr::editor::crop::title()),
                        )
                        .child(footprint)
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .gap(px(8.))
                                .child(button("cover-crop-reset", tr::common::reset()).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(crop) =
                                            this.sidebar.title_cards.layers.crop.as_mut()
                                        {
                                            crop.reset();
                                            cx.notify();
                                        }
                                    }),
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.))
                                        .child(
                                            button("cover-crop-cancel", tr::common::cancel())
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.close_cover_image_crop(false, window, cx)
                                                })),
                                        )
                                        .child(
                                            button(
                                                "cover-crop-apply",
                                                tr::editor::cover_controls::apply(),
                                            )
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.close_cover_image_crop(true, window, cx)
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                )
                .children(crop.state.drag.map(|drag| {
                    ui::Slider::drag_layer(
                        "cover-crop-drag",
                        cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                            if let Some(crop) = this.sidebar.title_cards.layers.crop.as_mut() {
                                crop.move_pointer(event);
                                cx.notify();
                            }
                        }),
                        cx.listener(|this, _, _, cx| {
                            if let Some(crop) = this.sidebar.title_cards.layers.crop.as_mut() {
                                crop.release();
                                cx.notify();
                            }
                        }),
                    )
                    .cursor(drag.cursor())
                }))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_crop_round_trips_between_source_and_normalized_coordinates() {
        let source = (800, 1600);
        let crop = [0.25, 0.0625, 0.5, 0.25];
        let rect = source_bounds(crop, source);
        assert_eq!(rect, CropBounds::new(200., 100., 400., 400.));
        assert_eq!(normalized_bounds(rect, source), crop);
    }
}
