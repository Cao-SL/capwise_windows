//! Things placed on top of a cover: words, people, stickers and pictures.
//! They sit over whatever the cover shows -- a template, a generated
//! background or a finished cover image -- and move and scale on the canvas.

mod text_style;

use text_style::CoverTextDialog;

use std::path::{Path, PathBuf};

use cap_project::{CoverLayer, CoverLayerKind, CoverPersonaRef, CoverTextStyle, LayerPose, XY};
use gpui::{
    AnyElement, AppContext as _, AsyncWindowContext, Context, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, StatefulInteractiveElement, Styled, StyledImage,
    Window, div, img, prelude::FluentBuilder, px, rgb,
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

/// Fonts that suit a cover's big words, the default first.
const COVER_FONTS: &[&str] = &[
    "Noto Sans SC",
    "Smiley Sans",
    "宋骨黑体",
    "LXGW WenKai Lite",
    "Noto Serif SC",
];

#[derive(Default)]
pub(crate) struct CoverLayerUi {
    /// The persona grid is open to add a person as a layer.
    adding_person: Option<String>,
    busy: bool,
    pub(crate) text_dialog: Option<CoverTextDialog>,
    pub(crate) crop: Option<crate::editor_cover_crop::CoverCrop>,
    pub(crate) menu: Option<(CanvasSelection, ui::MenuState)>,
    /// Pixel sizes of placed people, read once from their files.
    sizes: std::cell::RefCell<std::collections::HashMap<String, Option<(u32, u32)>>>,
}

impl CoverLayerUi {
    pub(crate) fn close_person_picker(&mut self) {
        self.adding_person = None;
    }

    /// Opens the person picker on one persona.
    pub(crate) fn show_persona(&mut self, id: &str) {
        self.adding_person = Some(id.to_owned());
    }
}

impl crate::editor_title_cards::TitleCardUi {
    pub(crate) fn layers_busy(&self) -> bool {
        self.layers.busy
    }

    pub(crate) fn set_layers_busy(&mut self, busy: bool) {
        self.layers.busy = busy;
    }
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
    pub(crate) fn cover_card(&self, index: usize) -> Option<&cap_project::TextSegment> {
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

    pub(crate) fn cover_layer(&self, index: usize, layer: usize) -> Option<&CoverLayer> {
        self.cover_card(index)?
            .title_card
            .as_ref()?
            .layers
            .get(layer)
    }

    pub(crate) fn is_cover_picture(&self, element: CanvasSelection) -> bool {
        match element {
            CanvasSelection::CoverLayer(index, layer) => self
                .cover_layer(index, layer)
                .is_some_and(|item| matches!(item.kind, CoverLayerKind::Image { .. })),
            _ => false,
        }
    }

    pub(crate) fn cover_element_locked(&self, element: CanvasSelection) -> bool {
        match element {
            CanvasSelection::CoverLayer(index, layer) => self
                .cover_layer(index, layer)
                .is_some_and(|item| item.locked),
            _ => false,
        }
    }

    pub(crate) fn open_cover_layer_menu(
        &mut self,
        element: CanvasSelection,
        point: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let items = self.cover_layer_menu_items(element);
        self.sidebar.title_cards.layers.menu = Some((element, ui::MenuState::new(point, &items)));
        cx.notify();
    }

    fn cover_layer_menu_items(&self, element: CanvasSelection) -> Vec<ui::MenuItem> {
        if self.cover_element_locked(element) {
            return vec![ui::MenuItem::new(
                tr::editor::cover_controls::unlock(),
                false,
            )];
        }
        vec![
            ui::MenuItem::new(tr::editor::crop::title(), false),
            ui::MenuItem::new(tr::editor::cover_controls::lock(), false),
        ]
    }

    pub(crate) fn render_cover_layer_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (element, state) = self.sidebar.title_cards.layers.menu.as_ref()?;
        let element = *element;
        let locked = self.cover_element_locked(element);
        self.is_cover_picture(element).then(|| {
            ui::Menu::editor(
                &self.theme,
                "cover-layer-menu",
                self.cover_layer_menu_items(element),
                state,
            )
            .on_select(cx.listener(move |this, choice, window, cx| {
                this.sidebar.title_cards.layers.menu = None;
                if !locked && *choice == 0 {
                    this.open_cover_image_crop(element, window, cx);
                } else {
                    this.toggle_cover_lock(element, window, cx);
                }
                cx.notify();
            }))
            .on_dismiss(cx.listener(|this, _, _, cx| {
                this.sidebar.title_cards.layers.menu = None;
                cx.notify();
            }))
            .into_any_element()
        })
    }

    fn toggle_cover_lock(
        &mut self,
        element: CanvasSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let CanvasSelection::CoverLayer(index, layer) = element
            && self.is_cover_picture(element)
        {
            self.edit_cover_layer(index, layer, window, cx, |layers, layer| {
                layers[layer].locked = !layers[layer].locked;
            });
            if self.cover_element_locked(element) && self.canvas_selection == Some(element) {
                self.canvas_selection = None;
            }
        }
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
                let item = card.layers.get(layer)?;
                let pose = match &item.kind {
                    CoverLayerKind::Image {
                        path,
                        persona: Some(_),
                        ..
                    } => {
                        let size = *self
                            .sidebar
                            .title_cards
                            .layers
                            .sizes
                            .borrow_mut()
                            .entry(path.clone())
                            .or_insert_with(|| {
                                image::image_dimensions(self.project_path.join(path)).ok()
                            });
                        size.map_or(item.pose, |size| {
                            item.display_pose(size, self.cover_output())
                        })
                    }
                    _ => item.pose,
                };
                Some(pose_rect(&pose))
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
        if self.cover_element_locked(element)
            || ![center.x, center.y, size.x, size.y]
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

    pub(crate) fn cover_output(&self) -> (u32, u32) {
        self.frame_layout
            .as_ref()
            .map(|layout| (layout.output_size[0], layout.output_size[1]))
            .filter(|size| size.0 > 0 && size.1 > 0)
            .unwrap_or((1920, 1080))
    }

    pub(crate) fn card_index_by_id(&self, card_id: &str) -> Option<usize> {
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
        let layer_id = layer.id.clone();
        self.update_title_card(index, window, cx, move |segment| {
            segment.title_card.as_mut().unwrap().layers.push(layer);
        });
        if self
            .selected_title_card()
            .is_some_and(|(current, _)| current == index)
            && let Some(layer) = self
                .cover_card(index)
                .and_then(|segment| segment.title_card.as_ref())
                .and_then(|card| card.layers.iter().position(|item| item.id == layer_id))
        {
            self.show_cover_on_canvas(index, cx);
            self.canvas_selection = Some(CanvasSelection::CoverLayer(index, layer));
        }
        cx.notify();
    }

    pub(crate) fn title_card_at_playhead(&self, index: usize) -> bool {
        self.cover_card(index).is_some_and(|segment| {
            let time = self.playhead_time();
            time >= segment.start && time < segment.end
        })
    }

    pub(crate) fn show_cover_on_canvas(&mut self, index: usize, cx: &mut Context<Self>) {
        let time = self.cover_card(index).and_then(|segment| {
            crate::editor_title_cards::card_preview_time(
                segment,
                crate::editor_window::EDITOR_PREVIEW_FPS,
            )
        });
        if let Some(time) = time {
            if self.playing {
                self.stop_playback(cx);
            }
            let cleared_preview = self.clear_timeline_preview();
            if !self.title_card_at_playhead(index) {
                self.seek_to_time(time, cx);
            } else if cleared_preview {
                self.publish_project();
                cx.notify();
            }
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
                font: None,
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
                                persona: None,
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
        let reference = CoverPersonaRef {
            persona: persona.id.clone(),
            file: file.clone(),
        };
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
                                persona: Some(reference),
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

    /// Puts another pose of the same person in a layer's place, standing
    /// where the old one stood.
    fn swap_cover_pose(
        &mut self,
        index: usize,
        layer: usize,
        persona: Persona,
        file: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let Some(card_id) = self.card_id_at(index) else {
            return;
        };
        let Some(layer_id) = self.cover_layer(index, layer).map(|item| item.id.clone()) else {
            return;
        };
        let project_path = self.project_path.clone();
        let reference = CoverPersonaRef {
            persona: persona.id.clone(),
            file,
        };
        self.sidebar.title_cards.layers.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let placed = cx
                .background_executor()
                .spawn(async move { personas::place_in_project(&project_path, &persona, &pose) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.layers.busy = false;
                match placed {
                    Ok((record, size)) => {
                        this.remember_project_asset(record.clone(), cx);
                        let Some(index) = this.card_index_by_id(&card_id) else {
                            return;
                        };
                        let output = this.cover_output();
                        this.update_title_card(index, window, cx, move |segment| {
                            let Some(item) = segment
                                .title_card
                                .as_mut()
                                .unwrap()
                                .layers
                                .iter_mut()
                                .find(|item| item.id == layer_id && !item.locked)
                            else {
                                return;
                            };
                            if let CoverLayerKind::Image { path, persona, .. } = &mut item.kind {
                                *path = record.path;
                                *persona = Some(reference);
                                item.pose = item.display_pose(size, output);
                            }
                        });
                    }
                    Err(error) => this.title_card_error(error, window, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn edit_cover_layer(
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

    /// Opens a text layer for typing on the canvas. Returns false for a
    /// picture, which has nothing to type.
    pub(crate) fn begin_cover_text_edit(
        &mut self,
        index: usize,
        layer: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(segment) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
        else {
            return false;
        };
        let Some(item) = segment
            .title_card
            .as_ref()
            .and_then(|card| card.layers.get(layer))
        else {
            return false;
        };
        let title = matches!(item.kind, CoverLayerKind::Title { .. });
        let before = match &item.kind {
            CoverLayerKind::Title { .. } => segment.content.clone(),
            CoverLayerKind::Text { content, .. } => content.clone(),
            CoverLayerKind::Image { .. } => return false,
        };
        self.stop_canvas_text_edit(cx);
        self.end_field_edit(cx);
        let input = cx.new(|cx| crate::ui::TextInputState::multi_line(window, cx));
        input.update(cx, |input, cx| input.set_text(before.clone(), cx));
        let write = move |this: &mut Self,
                          text: String,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
            if title {
                this.edit_text_segment("cover-title-content", index, window, cx, move |segment| {
                    if segment.content == text {
                        return false;
                    }
                    segment.content = text;
                    true
                });
            } else if let Some(CoverLayerKind::Text { content, .. }) = this
                .project
                .timeline
                .as_mut()
                .and_then(|timeline| timeline.text_segments.get_mut(index))
                .and_then(|segment| segment.title_card.as_mut())
                .and_then(|card| card.layers.get_mut(layer))
                .map(|item| &mut item.kind)
                && *content != text
            {
                *content = text;
                this.project_changed_live(cx);
                this.schedule_save(window, cx);
            }
        };
        let subscription =
            cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                if this
                    .canvas_text_edit
                    .as_ref()
                    .is_none_or(|edit| edit.index != index || edit.layer != Some(layer))
                {
                    return;
                }
                match event {
                    crate::ui::TextInputEvent::Changed => {
                        let mut text = input.read(cx).text().to_owned();
                        if title && text.trim().is_empty() {
                            text = this.canvas_text_edit.as_ref().unwrap().before.clone();
                        }
                        write(this, text, window, cx);
                    }
                    crate::ui::TextInputEvent::Cancelled => {
                        let before = this.canvas_text_edit.as_ref().unwrap().before.clone();
                        write(this, before, window, cx);
                        this.stop_canvas_text_edit(cx);
                        this.focus_root(window, cx);
                    }
                    crate::ui::TextInputEvent::Blurred | crate::ui::TextInputEvent::Confirmed => {
                        this.stop_canvas_text_edit(cx);
                    }
                }
            });
        self.history.pause();
        self.canvas_text_edit = Some(crate::editor_canvas::CanvasTextEdit {
            index,
            layer: Some(layer),
            before,
            input: input.clone(),
            _subscription: subscription,
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.publish_project();
        cx.notify();
        true
    }

    /// The typing box over a text layer, sized like the words under it.
    pub(crate) fn render_cover_text_input(
        &self,
        element: CanvasSelection,
        rect: crate::editor_canvas::NormRect,
        canvas: (f32, f32),
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(edit) = self.canvas_text_edit.as_ref() else {
            return div().into_any_element();
        };
        let CanvasSelection::CoverLayer(index, layer) = element else {
            return div().into_any_element();
        };
        let item = self.cover_layer(index, layer);
        let style = item.and_then(|item| item.kind.text_style());
        let lines = edit.before.lines().count().max(1) as f32;
        let height = rect.h as f32 * canvas.1;
        let font_size = item
            .and_then(|item| item.font_size)
            .map_or(height / lines / 1.15 * 0.86, |size| {
                size * canvas.1 / 1080.0
            })
            .min(height / lines / 1.15)
            .max(10.0);
        let color = style
            .as_ref()
            .and_then(|style| u32::from_str_radix(style.color.trim_start_matches('#'), 16).ok())
            .map(|value| Hsla::from(rgb(value)))
            .unwrap_or_else(gpui::white);
        let band = style
            .as_ref()
            .and_then(|style| style.band.as_ref())
            .and_then(|band| u32::from_str_radix(band.trim_start_matches('#'), 16).ok())
            .map(|value| Hsla::from(rgb(value)))
            .unwrap_or_else(|| gpui::hsla(0., 0., 0., 0.55));
        let family = style
            .and_then(|style| style.font)
            .unwrap_or_else(|| COVER_FONTS[0].to_owned());
        div()
            .id("cover-text-editor")
            .absolute()
            .left(px(rect.x as f32 * canvas.0))
            .top(px(rect.y as f32 * canvas.1))
            .w(px(rect.w as f32 * canvas.0))
            .h(px(height.max(font_size * 1.3)))
            .occlude()
            .rounded(px(4.))
            .bg(band)
            .border_1()
            .border_color(Hsla::from(self.theme.blue_9))
            .font_family(gpui::SharedString::from(family))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                crate::ui::TextInput::bare(&self.theme, "cover-text-inline-input", &edit.input)
                    .height(px(height.max(font_size * 1.3)))
                    .padding_x(px(0.))
                    .padding_y(px(0.))
                    .text_size(px(font_size))
                    .line_height(px(font_size * 1.15))
                    .font_weight(gpui::FontWeight::BLACK)
                    .align(gpui::TextAlign::Center)
                    .text_color(color)
                    .caret_color(color)
                    .selection_color(gpui::hsla(0.58, 0.9, 0.6, 0.35)),
            )
            .into_any_element()
    }

    /// Deletes the cover element selected on the canvas. Returns whether one
    /// was selected.
    pub(crate) fn delete_cover_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.canvas_selection {
            Some(element @ CanvasSelection::CoverLayer(index, layer)) => {
                if self.cover_element_locked(element) {
                    return true;
                }
                self.canvas_selection = None;
                self.edit_cover_layer(index, layer, window, cx, |layers, layer| {
                    layers.remove(layer);
                });
                true
            }
            Some(CanvasSelection::CoverSubject(index)) => {
                self.canvas_selection = None;
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
            _ => None,
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
            .flex_wrap()
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
            let chips =
                div()
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
                    }))
                    .child(
                        ui::EditorButton::plain(&theme, "cover-persona-upload")
                            .outlined(&theme)
                            .height(px(28.))
                            .text_size(px(12.))
                            .left_icon("icons/image.svg")
                            .icon_size(px(12.))
                            .label(if self.sidebar.title_cards.subject.busy {
                                tr::editor::title_cards::subject_busy()
                            } else {
                                tr::editor::title_cards::subject_upload()
                            })
                            .disabled(busy || self.sidebar.title_cards.subject.busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.import_persona_photo(window, cx)
                            })),
                    );
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
        let selected_index = selected.as_ref().map(|(layer, _)| *layer);
        let editor =
            selected.map(|(layer, item)| self.render_cover_layer_editor(index, layer, &item, cx));
        let elements = (!card.layers.is_empty()).then(|| {
            ui::Field::stacked(&theme, tr::editor::cover_templates::elements()).child(
                div().flex().flex_wrap().gap(px(6.)).children(
                    card.layers.iter().enumerate().rev().map(|(layer, item)| {
                        let fallback = match &item.kind {
                            CoverLayerKind::Title { .. } | CoverLayerKind::Text { .. } => {
                                tr::editor::title_cards::add_text()
                            }
                            CoverLayerKind::Image {
                                persona: Some(_), ..
                            } => tr::editor::title_cards::add_person(),
                            CoverLayerKind::Image { .. } => tr::editor::title_cards::add_image(),
                        };
                        let text = match &item.kind {
                            CoverLayerKind::Title { .. } => self
                                .cover_card(index)
                                .map(|segment| segment.content.as_str()),
                            CoverLayerKind::Text { content, .. } => Some(content.as_str()),
                            _ => None,
                        };
                        let label = text
                            .and_then(|text| text.trim().lines().next())
                            .filter(|text| !text.is_empty())
                            .unwrap_or(fallback);
                        let mut short = label.chars().take(18).collect::<String>();
                        if short.len() < label.len() {
                            short.push('…');
                        }
                        ui::EditorButton::plain(&theme, ("select-cover-layer", layer))
                            .choice(&theme)
                            .height(px(28.))
                            .text_size(px(12.))
                            .label(short)
                            .when(item.locked, |pill| pill.left_icon("icons/lock.svg"))
                            .tooltip(&theme, label.to_owned())
                            .pressed(selected_index == Some(layer))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.stop_canvas_text_edit(cx);
                                this.end_field_edit(cx);
                                this.show_cover_on_canvas(index, cx);
                                this.canvas_selection =
                                    Some(CanvasSelection::CoverLayer(index, layer));
                                this.focus_root(window, cx);
                                cx.notify();
                            }))
                    }),
                ),
            )
        });

        ui::Field::stacked(&theme, tr::editor::title_cards::add())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(add_row)
                    .children(person_grid)
                    .children(elements)
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
        let element = CanvasSelection::CoverLayer(index, layer);
        if item.locked {
            return body
                .child(
                    ui::EditorButton::plain(&theme, "cover-unlock")
                        .outlined(&theme)
                        .left_icon("icons/lock-open.svg")
                        .label(tr::editor::cover_controls::unlock())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_cover_lock(element, window, cx);
                        })),
                )
                .into_any_element();
        }
        if let Some(style) = item.kind.text_style() {
            let field = if matches!(item.kind, CoverLayerKind::Title { .. }) {
                FieldKey::TextContent(index)
            } else {
                FieldKey::CoverLayerText(index, layer)
            };
            let hex = |value: &str| {
                rgb(u32::from_str_radix(value.trim_start_matches('#'), 16).unwrap_or(0))
            };
            let styles = TEXT_STYLES
                .iter()
                .enumerate()
                .map(|(slot, (fill, outline, back))| {
                    let current = style.color == *fill
                        && style.stroke.as_deref() == *outline
                        && style.band.as_deref() == *back
                        && item.stroke_width.is_none()
                        && item.band_opacity.is_none();
                    let (fill, outline, back) = (*fill, *outline, *back);
                    let font = style.font.clone();
                    div()
                        .id(("cover-text-style", slot))
                        .h(px(30.))
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(back.map_or(gpui::hsla(0., 0., 0.42, 1.), |back| hex(back).into()))
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
                            let next = CoverTextStyle {
                                color: fill.into(),
                                stroke: outline.map(Into::into),
                                band: back.map(Into::into),
                                font: font.clone(),
                            };
                            this.edit_cover_layer(
                                index,
                                layer,
                                window,
                                cx,
                                move |layers, layer| {
                                    layers[layer].kind.set_text_style(next);
                                    layers[layer].stroke_width = None;
                                    layers[layer].band_opacity = None;
                                },
                            );
                        }))
                });
            let current_font = style.font.clone().unwrap_or_else(|| COVER_FONTS[0].into());
            let fonts = COVER_FONTS
                .iter()
                .filter(|family| cap_rendering::font_family_available(family))
                .map(|family| {
                    let family = *family;
                    let style = style.clone();
                    ui::EditorButton::plain(
                        &theme,
                        gpui::ElementId::Name(format!("cover-font-{family}").into()),
                    )
                    .choice(&theme)
                    .height(px(26.))
                    .text_size(px(12.))
                    .label(crate::editor_tabs::font_option_label(family))
                    .pressed(current_font == family)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut next = style.clone();
                        next.font = (family != COVER_FONTS[0]).then(|| family.to_owned());
                        this.edit_cover_layer(index, layer, window, cx, move |layers, layer| {
                            layers[layer].kind.set_text_style(next);
                        });
                    }))
                });
            body = body
                .child(self.render_field_input(field, Some(52.)))
                .child(div().flex().gap(px(6.)).children(styles))
                .child(div().flex().flex_wrap().gap(px(6.)).children(fonts));
            body = body.child(
                ui::EditorButton::plain(&theme, "cover-custom-text")
                    .outlined(&theme)
                    .label(tr::editor::cover_controls::custom())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_cover_text_dialog(index, layer, window, cx);
                    })),
            );
        }
        if let CoverLayerKind::Image {
            persona: Some(reference),
            ..
        } = &item.kind
            && let Some(persona) = self
                .sidebar
                .title_cards
                .subject
                .persona_list()
                .into_iter()
                .find(|persona| persona.id == reference.persona)
        {
            let busy = self.sidebar.title_cards.layers.busy;
            let outfit = persona
                .manifest
                .poses
                .iter()
                .find(|pose| pose.file == reference.file)
                .map(|pose| pose.outfit.clone());
            let grid = div().grid().grid_cols(6).gap(px(4.)).children(
                persona
                    .manifest
                    .poses
                    .iter()
                    .filter(|pose| outfit.as_ref().is_none_or(|outfit| &pose.outfit == outfit))
                    .filter_map(|pose| {
                        let path = persona.pose_path(pose)?;
                        let chosen = persona.clone();
                        let file = pose.file.clone();
                        let current = file == reference.file;
                        Some(
                            div()
                                .id(gpui::ElementId::Name(format!("cover-swap-{file}").into()))
                                .h(px(52.))
                                .rounded(px(6.))
                                .overflow_hidden()
                                .bg(Hsla::from(theme.editor.card))
                                .border_1()
                                .border_color(if current {
                                    Hsla::from(theme.editor.accent)
                                } else {
                                    Hsla::from(theme.editor.line)
                                })
                                .when(!busy && !current, |this| this.cursor_pointer())
                                .child(img(path).size_full().object_fit(gpui::ObjectFit::Contain))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if !current {
                                        this.swap_cover_pose(
                                            index,
                                            layer,
                                            chosen.clone(),
                                            file.clone(),
                                            window,
                                            cx,
                                        );
                                    }
                                })),
                        )
                    }),
            );
            body = body.child(
                ui::Field::stacked(&theme, tr::editor::cover_templates::change_pose()).child(grid),
            );
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
            body = body.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .child(
                        small("cover-layer-crop", tr::editor::crop::title()).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_cover_image_crop(element, window, cx)
                            },
                        )),
                    )
                    .child(
                        small("cover-layer-lock", tr::editor::cover_controls::lock()).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.toggle_cover_lock(element, window, cx)
                            }),
                        ),
                    ),
            );

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
