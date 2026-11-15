//! Things placed on top of a cover: words, people, stickers and pictures.
//! They sit over whatever the cover shows -- a template, a generated
//! background or a finished cover image -- and move and scale on the canvas.

use std::path::{Path, PathBuf};

use cap_project::{CoverLayer, CoverLayerKind, LayerPose, XY};
use gpui::{
    AnyElement, AsyncWindowContext, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder, px,
    rgb,
};

use crate::{
    editor_canvas::CanvasSelection,
    editor_panels::FieldKey,
    editor_window::EditorWindow,
    i18n::tr,
    personas::{self, Persona},
    ui,
};

/// Word styles that read on any picture: an outline or a filled band.
pub(crate) const TEXT_STYLES: &[(&str, Option<&str>, Option<&str>)] = &[
    ("#FFFFFF", Some("#111111"), None),
    ("#FACC15", Some("#111111"), None),
    ("#FFFFFF", None, Some("#EF4444")),
    ("#111111", None, Some("#FACC15")),
];

#[derive(Default)]
pub(crate) struct CoverLayerUi {
    /// The persona grid is open to add a person as a layer.
    adding_person: Option<String>,
    busy: bool,
    /// Pixel sizes of placed people, read once from their files.
    sizes: std::cell::RefCell<std::collections::HashMap<String, Option<(u32, u32)>>>,
}

/// A layer's box from its pose, normalised to the card.
pub(crate) fn pose_rect(pose: &LayerPose) -> crate::editor_canvas::NormRect {
    crate::editor_canvas::NormRect {
        x: pose.center.x - pose.size.x / 2.,
        y: pose.center.y - pose.size.y / 2.,
        w: pose.size.x,
        h: pose.size.y,
    }
}

/// A picture `height` tall (share of the card), keeping its own shape on a
/// card of `output` pixels.
fn picture_pose(
    center: (f64, f64),
    height: f64,
    source: (u32, u32),
    output: (u32, u32),
) -> LayerPose {
    let aspect = f64::from(source.0.max(1)) / f64::from(source.1.max(1));
    let card = f64::from(output.0.max(1)) / f64::from(output.1.max(1));
    let mut size = XY::new(height * aspect / card, height);
    if size.x > 0.9 {
        let scale = 0.9 / size.x;
        size = XY::new(size.x * scale, size.y * scale);
    }
    LayerPose {
        center: XY::new(center.0, center.1),
        size,
        rotation: 0.0,
        opacity: 1.0,
    }
}

fn read_layer_picture(source: &Path) -> Result<(Vec<u8>, String), String> {
    let bytes = std::fs::read(source).map_err(|error| error.to_string())?;
    let name = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("image")
        .to_owned();
    Ok((bytes, name))
}

impl EditorWindow {
    fn cover_card(&self, index: usize) -> Option<&cap_project::TextSegment> {
        self.project
            .timeline
            .as_ref()?
            .text_segments
            .get(index)
            .filter(|segment| segment.title_card.is_some())
    }

    /// The cover's person (when drawn) and its layers, bottom to top.
    pub(crate) fn cover_elements(&self, index: usize) -> Vec<CanvasSelection> {
        let Some(card) = self
            .cover_card(index)
            .and_then(|segment| segment.title_card.as_ref())
        else {
            return Vec::new();
        };
        let mut elements = Vec::new();
        if card.subject.is_some() && !card.hide_title {
            elements.push(CanvasSelection::CoverSubject(index));
        }
        elements
            .extend((0..card.layers.len()).map(|layer| CanvasSelection::CoverLayer(index, layer)));
        elements
    }

    /// People and pictures keep their shape when resized; text boxes do not.
    pub(crate) fn cover_keeps_shape(&self, element: CanvasSelection) -> bool {
        match element {
            CanvasSelection::CoverSubject(_) => true,
            CanvasSelection::CoverLayer(index, layer) => self
                .cover_card(index)
                .and_then(|segment| segment.title_card.as_ref()?.layers.get(layer))
                .is_some_and(|item| matches!(item.kind, CoverLayerKind::Image { .. })),
            _ => false,
        }
    }

    pub(crate) fn cover_element_rect(
        &self,
        element: CanvasSelection,
    ) -> Option<crate::editor_canvas::NormRect> {
        match element {
            CanvasSelection::CoverLayer(index, layer) => {
                let card = self.cover_card(index)?.title_card.as_ref()?;
                Some(pose_rect(&card.layers.get(layer)?.pose))
            }
            CanvasSelection::CoverSubject(index) => {
                let segment = self.cover_card(index)?;
                let subject = segment.title_card.as_ref()?.subject.as_ref()?;
                if let Some(pose) = &subject.pose {
                    return Some(pose_rect(pose));
                }
                let source = *self
                    .sidebar
                    .title_cards
                    .layers
                    .sizes
                    .borrow_mut()
                    .entry(subject.path.clone())
                    .or_insert_with(|| {
                        image::image_dimensions(self.project_path.join(&subject.path)).ok()
                    });
                let output = self.cover_output();
                let bounds = cap_rendering::title_card_subject_bounds(output, segment, source?)?;
                Some(crate::editor_canvas::NormRect {
                    x: f64::from(bounds[0]) / f64::from(output.0),
                    y: f64::from(bounds[1]) / f64::from(output.1),
                    w: f64::from(bounds[2] - bounds[0]) / f64::from(output.0),
                    h: f64::from(bounds[3] - bounds[1]) / f64::from(output.1),
                })
            }
            _ => None,
        }
    }

    /// Writes a dragged cover element's box. Returns whether it changed.
    pub(crate) fn write_cover_pose(
        &mut self,
        element: CanvasSelection,
        center: XY<f64>,
        size: XY<f64>,
    ) -> bool {
        if ![center.x, center.y, size.x, size.y]
            .iter()
            .all(|value| value.is_finite())
        {
            return false;
        }
        let Some(index) = element.cover_card() else {
            return false;
        };
        let Some(card) = self
            .project
            .timeline
            .as_mut()
            .and_then(|timeline| timeline.text_segments.get_mut(index))
            .and_then(|segment| segment.title_card.as_mut())
        else {
            return false;
        };
        let pose = match element {
            CanvasSelection::CoverLayer(_, layer) => match card.layers.get_mut(layer) {
                Some(item) => &mut item.pose,
                None => return false,
            },
            CanvasSelection::CoverSubject(_) => match card.subject.as_mut() {
                Some(subject) if subject.pose.is_none() => {
                    subject.pose = Some(LayerPose {
                        center,
                        size,
                        rotation: 0.0,
                        opacity: 1.0,
                    });
                    return true;
                }
                Some(subject) => subject.pose.as_mut().unwrap(),
                None => return false,
            },
            _ => return false,
        };
        if pose.center == center && pose.size == size {
            return false;
        }
        pose.center = center;
        pose.size = size;
        true
    }

    fn cover_output(&self) -> (u32, u32) {
        self.frame_layout
            .as_ref()
            .map(|layout| (layout.output_size[0], layout.output_size[1]))
            .filter(|size| size.0 > 0 && size.1 > 0)
            .unwrap_or((1920, 1080))
    }

    fn card_index_by_id(&self, card_id: &str) -> Option<usize> {
        self.project
            .timeline
            .as_ref()?
            .text_segments
            .iter()
            .position(|segment| {
                segment
                    .title_card
                    .as_ref()
                    .is_some_and(|card| card.id == card_id)
            })
    }

    fn card_id_at(&self, index: usize) -> Option<String> {
        self.project
            .timeline
            .as_ref()?
            .text_segments
            .get(index)?
            .title_card
            .as_ref()
            .map(|card| card.id.clone())
    }

    /// Appends a layer to the card and selects it on the canvas.
    fn push_cover_layer(
        &mut self,
        card_id: &str,
        layer: CoverLayer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.card_index_by_id(card_id) else {
            self.title_card_error(tr::editor::title_cards::changed().into(), window, cx);
            return;
        };
        let full = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.as_ref())
            .is_some_and(|card| card.layers.len() >= cap_project::MAX_COVER_LAYERS);
        if full {
            self.title_card_error(tr::editor::title_cards::layers_full().into(), window, cx);
            return;
        }
        self.update_title_card(index, window, cx, move |segment| {
            segment.title_card.as_mut().unwrap().layers.push(layer);
        });
        let count = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.as_ref())
            .map_or(0, |card| card.layers.len());
        if count > 0 {
            self.show_cover_on_canvas(index, cx);
            self.canvas_selection = Some(CanvasSelection::CoverLayer(index, count - 1));
        }
        cx.notify();
    }

    /// Moves the playhead onto the card so its layers are on the canvas.
    fn show_cover_on_canvas(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((start, end)) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .map(|segment| (segment.start, segment.end))
        else {
            return;
        };
        let time = self.playhead_time();
        if time < start || time >= end {
            self.seek_to_time(start + ((end - start) * 0.5).min(0.1), cx);
        }
    }

    pub(crate) fn add_cover_text(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card_id) = self.card_id_at(index) else {
            return;
        };
        let (color, stroke, band) = TEXT_STYLES[1];
        let layer = CoverLayer::new(
            LayerPose {
                center: XY::new(0.5, 0.2),
                size: XY::new(0.56, 0.14),
                rotation: 0.0,
                opacity: 1.0,
            },
            CoverLayerKind::Text {
                content: tr::editor::title_cards::layer_text_default().into(),
                color: color.into(),
                stroke: stroke.map(Into::into),
                band: band.map(Into::into),
            },
        );
        self.push_cover_layer(&card_id, layer, window, cx);
    }

    /// Copies a picture into the project and puts it on the card.
    pub(crate) fn add_cover_picture(
        &mut self,
        card_id: String,
        bytes: Vec<u8>,
        name: String,
        placement: (f64, f64, f64),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project_path = self.project_path.clone();
        let output = self.cover_output();
        self.sidebar.title_cards.layers.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let saved = cx
                .background_executor()
                .spawn(async move { personas::save_layer_image(&project_path, bytes, name) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.layers.busy = false;
                match saved {
                    Ok((record, size)) => {
                        this.remember_project_asset(record.clone(), cx);
                        let (x, y, height) = placement;
                        let mut pose = picture_pose((x, y), height, size, output);
                        // A person stands on the bottom edge.
                        if y >= 1.0 {
                            pose.center.y = 1.0 - pose.size.y / 2.;
                        }
                        let layer = CoverLayer::new(
                            pose,
                            CoverLayerKind::Image {
                                path: record.path,
                                mirror: false,
                            },
                        );
                        this.push_cover_layer(&card_id, layer, window, cx);
                    }
                    Err(error) => this.title_card_error(error, window, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn pick_cover_picture(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card_id) = self.card_id_at(index) else {
            return;
        };
        if self.sidebar.title_cards.layers.busy {
            return;
        }
        self.sidebar.title_cards.layers.busy = true;
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let filters: [(&str, &[&str]); 1] = [(
                tr::editor::window::image::filter(),
                &["png", "jpg", "jpeg", "webp", "gif"],
            )];
            #[cfg(target_os = "linux")]
            let source = crate::platform::open_file_panel_async(&filters, None, cx).await;
            #[cfg(not(target_os = "linux"))]
            let source = crate::import::pick_import_file(&filters, cx).await;
            let read = match source {
                Some(source) => {
                    let source: PathBuf = source;
                    Some(
                        cx.background_executor()
                            .spawn(async move { read_layer_picture(&source) })
                            .await,
                    )
                }
                None => None,
            };
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.layers.busy = false;
                match read {
                    Some(Ok((bytes, name))) => {
                        this.add_cover_picture(card_id, bytes, name, (0.5, 0.5, 0.4), window, cx)
                    }
                    Some(Err(error)) => this.title_card_error(error, window, cx),
                    None => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn add_cover_person(
        &mut self,
        index: usize,
        persona: Persona,
        file: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(card_id) = self.card_id_at(index) else {
            return;
        };
        let Some(pose) = persona
            .manifest
            .poses
            .iter()
            .find(|pose| pose.file == file)
            .cloned()
        else {
            return;
        };
        if self.sidebar.title_cards.layers.busy {
            return;
        }
        let project_path = self.project_path.clone();
        let output = self.cover_output();
        self.sidebar.title_cards.layers.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let facing = pose.facing;
            let placed = cx
                .background_executor()
                .spawn(async move { personas::place_in_project(&project_path, &persona, &pose) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.layers.busy = false;
                match placed {
                    Ok((record, size)) => {
                        this.remember_project_asset(record.clone(), cx);
                        let mut pose = picture_pose((0.76, 1.0), 0.86, size, output);
                        pose.center.y = 1.0 - pose.size.y / 2.;
                        pose.center.x = (1.0 - pose.size.x / 2.).min(0.76).max(pose.size.x / 2.);
                        let layer = CoverLayer::new(
                            pose,
                            CoverLayerKind::Image {
                                path: record.path,
                                // Standing on the right, they look into the card.
                                mirror: personas::mirror_for(
                                    facing,
                                    cap_project::TitleCardSubjectSide::Right,
                                ),
                            },
                        );
                        this.sidebar.title_cards.layers.adding_person = None;
                        this.push_cover_layer(&card_id, layer, window, cx);
                    }
                    Err(error) => this.title_card_error(error, window, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn edit_cover_layer(
        &mut self,
        index: usize,
        layer: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Vec<CoverLayer>, usize) + 'static,
    ) {
        self.update_title_card(index, window, cx, move |segment| {
            let layers = &mut segment.title_card.as_mut().unwrap().layers;
            if layer < layers.len() {
                change(layers, layer);
            }
        });
        cx.notify();
    }

    pub(crate) fn commit_cover_layer_text(
        &mut self,
        index: usize,
        layer: usize,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_cover_layer(index, layer, window, cx, move |layers, layer| {
            if let CoverLayerKind::Text { content, .. } = &mut layers[layer].kind {
                *content = text;
            }
        });
    }

    /// Deletes the cover element selected on the canvas. Returns whether one
    /// was selected.
    pub(crate) fn delete_cover_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.canvas_selection {
            Some(CanvasSelection::CoverLayer(index, layer)) => {
                self.canvas_selection = None;
                self.edit_cover_layer(index, layer, window, cx, |layers, layer| {
                    layers.remove(layer);
                });
                true
            }
            Some(CanvasSelection::CoverSubject(index)) => {
                self.canvas_selection = None;
                self.sidebar.title_cards.subject.forget_placed();
                self.update_title_card(index, window, cx, |segment| {
                    segment.title_card.as_mut().unwrap().subject = None;
                });
                true
            }
            _ => false,
        }
    }

    /// The text currently in a text layer, for its field.
    pub(crate) fn cover_layer_text(&self, index: usize, layer: usize) -> Option<&str> {
        let card = self
            .project
            .timeline
            .as_ref()?
            .text_segments
            .get(index)?
            .title_card
            .as_ref()?;
        match &card.layers.get(layer)?.kind {
            CoverLayerKind::Text { content, .. } => Some(content),
            CoverLayerKind::Image { .. } => None,
        }
    }

    pub(crate) fn render_cover_layers(
        &self,
        index: usize,
        card: &cap_project::TitleCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let busy = self.sidebar.title_cards.layers.busy || self.asset_import_busy();
        let full = card.layers.len() >= cap_project::MAX_COVER_LAYERS;
        let button = |id: &'static str, icon: &'static str, label: &'static str| {
            ui::EditorButton::plain(&theme, id)
                .outlined(&theme)
                .grow()
                .height(px(32.))
                .text_size(px(12.))
                .left_icon(icon)
                .icon_size(px(13.))
                .label(label)
                .disabled(busy || full)
        };
        let adding_person = self.sidebar.title_cards.layers.adding_person.clone();
        let add_row = div()
            .flex()
            .gap(px(6.))
            .child(
                button(
                    "cover-add-text",
                    "icons/type.svg",
                    tr::editor::title_cards::add_text(),
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.add_cover_text(index, window, cx)),
                ),
            )
            .child(
                button(
                    "cover-add-person",
                    "icons/user-round.svg",
                    tr::editor::title_cards::add_person(),
                )
                .pressed(adding_person.is_some())
                .on_click(cx.listener(move |this, _, _, cx| {
                    let state = &mut this.sidebar.title_cards.layers;
                    state.adding_person = if state.adding_person.is_some() {
                        None
                    } else {
                        this.sidebar
                            .title_cards
                            .subject
                            .persona_ids()
                            .into_iter()
                            .next()
                            .or(Some(String::new()))
                    };
                    cx.notify();
                })),
            )
            .child(
                button(
                    "cover-add-sticker",
                    "icons/smile.svg",
                    tr::editor::title_cards::add_sticker(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(card_id) = this.card_id_at(index) {
                        this.open_cover_sticker_picker(card_id, window, cx);
                    }
                })),
            )
            .child(
                button(
                    "cover-add-image",
                    "icons/image.svg",
                    tr::editor::title_cards::add_image(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_cover_picture(index, window, cx)
                })),
            );

        let person_grid = adding_person.map(|browsing| {
            let personas = self.sidebar.title_cards.subject.persona_list();
            let chips = div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(personas.iter().map(|persona| {
                    let id = persona.id.clone();
                    let name = if persona.builtin {
                        persona.display_name().to_owned()
                    } else {
                        tr::editor::title_cards::subject_mine().to_string()
                    };
                    ui::EditorButton::plain(
                        &theme,
                        gpui::ElementId::Name(format!("cover-persona-{id}").into()),
                    )
                    .choice(&theme)
                    .height(px(28.))
                    .text_size(px(12.))
                    .label(name)
                    .pressed(browsing == id)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sidebar.title_cards.layers.adding_person = Some(id.clone());
                        cx.notify();
                    }))
                }));
            let grid = personas
                .iter()
                .find(|persona| persona.id == browsing)
                .or_else(|| personas.first())
                .map(|persona| {
                    div().grid().grid_cols(6).gap(px(4.)).children(
                        persona.manifest.poses.iter().filter_map(|pose| {
                            let path = persona.pose_path(pose)?;
                            let chosen = persona.clone();
                            let file = pose.file.clone();
                            Some(
                                div()
                                    .id(gpui::ElementId::Name(
                                        format!("cover-pose-{}-{file}", persona.id).into(),
                                    ))
                                    .h(px(52.))
                                    .rounded(px(6.))
                                    .overflow_hidden()
                                    .bg(Hsla::from(theme.editor.card))
                                    .border_1()
                                    .border_color(Hsla::from(theme.editor.line))
                                    .when(!busy, |this| {
                                        this.cursor_pointer().hover(|this| {
                                            this.border_color(Hsla::from(theme.editor.accent))
                                        })
                                    })
                                    .child(
                                        img(path).size_full().object_fit(gpui::ObjectFit::Contain),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.add_cover_person(
                                            index,
                                            chosen.clone(),
                                            file.clone(),
                                            window,
                                            cx,
                                        );
                                    })),
                            )
                        }),
                    )
                });
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(chips)
                .children(grid)
        });

        let selected = match self.canvas_selection {
            Some(CanvasSelection::CoverLayer(card_index, layer)) if card_index == index => {
                card.layers.get(layer).map(|item| (layer, item.clone()))
            }
            _ => None,
        };
        let editor =
            selected.map(|(layer, item)| self.render_cover_layer_editor(index, layer, &item, cx));

        ui::Field::stacked(&theme, tr::editor::title_cards::add())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(add_row)
                    .children(person_grid)
                    .children(editor),
            )
            .into_any_element()
    }

    fn render_cover_layer_editor(
        &self,
        index: usize,
        layer: usize,
        item: &CoverLayer,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(8.))
            .bg(Hsla::from(theme.editor.ctl));
        match &item.kind {
            CoverLayerKind::Text {
                color,
                stroke,
                band,
                ..
            } => {
                body = body
                    .child(
                        self.render_field_input(FieldKey::CoverLayerText(index, layer), Some(52.)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .children(TEXT_STYLES.iter().enumerate().map(
                                |(style, (fill, outline, back))| {
                                    let current = color == fill
                                        && stroke.as_deref() == *outline
                                        && band.as_deref() == *back;
                                    let hex = |value: &str| {
                                        rgb(u32::from_str_radix(value.trim_start_matches('#'), 16)
                                            .unwrap_or(0))
                                    };
                                    let (fill, outline, back) = (*fill, *outline, *back);
                                    div()
                                        .id(("cover-text-style", style))
                                        .h(px(30.))
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(6.))
                                        .bg(back.map_or(gpui::hsla(0., 0., 0.42, 1.), |back| {
                                            hex(back).into()
                                        }))
                                        .border_2()
                                        .border_color(if current {
                                            Hsla::from(theme.editor.accent)
                                        } else {
                                            gpui::transparent_black()
                                        })
                                        .cursor_pointer()
                                        .text_size(px(14.))
                                        .font_weight(gpui::FontWeight::BLACK)
                                        .text_color(hex(fill))
                                        .child(tr::editor::title_cards::layer_style_sample())
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.edit_cover_layer(
                                                index,
                                                layer,
                                                window,
                                                cx,
                                                move |layers, layer| {
                                                    if let CoverLayerKind::Text {
                                                        color,
                                                        stroke,
                                                        band,
                                                        ..
                                                    } = &mut layers[layer].kind
                                                    {
                                                        *color = fill.into();
                                                        *stroke = outline.map(Into::into);
                                                        *band = back.map(Into::into);
                                                    }
                                                },
                                            );
                                        }))
                                },
                            )),
                    );
            }
            CoverLayerKind::Image { .. } => {}
        }
        let small = |id: &'static str, label: &'static str| {
            ui::EditorButton::plain(&theme, id)
                .outlined(&theme)
                .grow()
                .height(px(28.))
                .text_size(px(12.))
                .label(label)
        };
        let mut actions = div().flex().gap(px(6.));
        if matches!(item.kind, CoverLayerKind::Image { .. }) {
            actions = actions.child(
                small("cover-layer-flip", tr::editor::title_cards::layer_flip()).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.edit_cover_layer(index, layer, window, cx, |layers, layer| {
                            if let CoverLayerKind::Image { mirror, .. } = &mut layers[layer].kind {
                                *mirror = !*mirror;
                            }
                        });
                    }),
                ),
            );
        }
        actions = actions
            .child(
                small("cover-layer-front", tr::editor::title_cards::layer_front()).on_click(
                    cx.listener(move |this, _, window, cx| {
                        let last = this
                            .project
                            .timeline
                            .as_ref()
                            .and_then(|timeline| timeline.text_segments.get(index))
                            .and_then(|segment| segment.title_card.as_ref())
                            .map_or(0, |card| card.layers.len().saturating_sub(1));
                        this.edit_cover_layer(index, layer, window, cx, |layers, layer| {
                            let item = layers.remove(layer);
                            layers.push(item);
                        });
                        this.canvas_selection = Some(CanvasSelection::CoverLayer(index, last));
                    }),
                ),
            )
            .child(
                small(
                    "cover-layer-delete",
                    tr::editor::title_cards::layer_delete(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.canvas_selection = Some(CanvasSelection::CoverLayer(index, layer));
                    this.delete_cover_selection(window, cx);
                })),
            );
        body.child(actions).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_keeps_its_shape_and_fits_the_card() {
        let pose = picture_pose((0.5, 0.5), 0.4, (1000, 500), (1920, 1080));
        let pixels = (pose.size.x * 1920., pose.size.y * 1080.);
        assert!((pixels.0 / pixels.1 - 2.0).abs() < 1e-9);
        let wide = picture_pose((0.5, 0.5), 0.8, (4000, 500), (1080, 1920));
        assert!(wide.size.x <= 0.9 + 1e-9);
        assert!(((wide.size.x * 1080.) / (wide.size.y * 1920.) - 8.0).abs() < 1e-9);
    }
}
