//! Cover templates: a set of layers placed in one click. The title's words
//! always carry over; a person slot takes whoever the user picked for
//! covers, and is left out when they picked no one.

use cap_project::{
    CoverLayer, CoverLayerKind, CoverPersonaRef, LayerPose, TitleCardSubjectSide, XY,
};
use gpui::{AsyncWindowContext, Context, Window};

use crate::{
    editor_window::EditorWindow,
    i18n::tr,
    personas::{self, Expression, Persona},
};

const STORE_SECTION: &str = "capwise_cover";

/// Who a generated cover or a template puts on the cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CoverPersona {
    #[default]
    Hanfu,
    Mine,
    None,
}

/// How a generated cover is made: a plain background under our own layers,
/// or one finished image with the title and person painted in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CoverMode {
    #[default]
    Collage,
    Painted,
}

impl CoverPersona {
    const ALL: [Self; 3] = [Self::Hanfu, Self::Mine, Self::None];

    fn key(self) -> &'static str {
        match self {
            Self::Hanfu => "hanfu",
            Self::Mine => "mine",
            Self::None => "none",
        }
    }

    pub(crate) fn load() -> Self {
        crate::store::store_string(STORE_SECTION, "persona")
            .and_then(|key| Self::ALL.into_iter().find(|value| value.key() == key))
            .unwrap_or_default()
    }

    pub(crate) fn save(self) {
        crate::store::set_store_setting(STORE_SECTION, "persona", self.key().into());
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Hanfu => tr::editor::cover_templates::persona_hanfu(),
            Self::Mine => tr::editor::cover_templates::persona_mine(),
            Self::None => tr::editor::cover_templates::persona_none(),
        }
    }

    pub(crate) fn all() -> [Self; 3] {
        Self::ALL
    }

    /// The persona this choice stands for, when it has any poses.
    pub(crate) fn persona(self, personas: &[Persona]) -> Option<Persona> {
        let id = match self {
            Self::Hanfu => personas.iter().find(|persona| persona.builtin)?.id.clone(),
            Self::Mine => personas::PRIVATE_ID.to_owned(),
            Self::None => return None,
        };
        personas
            .iter()
            .find(|persona| persona.id == id && !persona.manifest.poses.is_empty())
            .cloned()
    }
}

impl CoverMode {
    pub(crate) fn load() -> Self {
        match crate::store::store_string(STORE_SECTION, "mode").as_deref() {
            Some("painted") => Self::Painted,
            _ => Self::Collage,
        }
    }

    pub(crate) fn save(self) {
        let key = match self {
            Self::Collage => "collage",
            Self::Painted => "painted",
        };
        crate::store::set_store_setting(STORE_SECTION, "mode", key.into());
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Style {
    color: &'static str,
    stroke: Option<&'static str>,
    band: Option<&'static str>,
}

const WHITE_OUTLINE: Style = Style {
    color: "#FFFFFF",
    stroke: Some("#111111"),
    band: None,
};
const YELLOW_OUTLINE: Style = Style {
    color: "#FACC15",
    stroke: Some("#111111"),
    band: None,
};
const RED_BAND: Style = Style {
    color: "#FFFFFF",
    stroke: None,
    band: Some("#EF4444"),
};
const YELLOW_BAND: Style = Style {
    color: "#111111",
    stroke: None,
    band: Some("#FACC15"),
};

#[derive(Clone, Copy)]
enum Slot {
    Title(Style),
    Text(&'static str, Style),
    /// A person standing on the bottom edge, `height` of the card tall.
    Person {
        side: TitleCardSubjectSide,
        expression: Expression,
    },
    Sticker(&'static str),
}

/// (centre x, centre y, width, height), all shares of the card.
type Box4 = (f64, f64, f64, f64);

#[derive(Clone, Copy)]
struct TemplateLayer {
    wide: Box4,
    tall: Box4,
    slot: Slot,
}

pub(crate) struct CoverTemplate {
    pub id: &'static str,
    name: fn() -> &'static str,
    layers: &'static [TemplateLayer],
}

impl CoverTemplate {
    pub(crate) fn name(&self) -> &'static str {
        (self.name)()
    }

    pub(crate) fn has_person(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| matches!(layer.slot, Slot::Person { .. }))
    }
}

const fn layer(wide: Box4, tall: Box4, slot: Slot) -> TemplateLayer {
    TemplateLayer { wide, tall, slot }
}

pub(crate) const HERO_CENTER: &str = "hero_center";
pub(crate) const TITLE_LEFT_PERSON_RIGHT: &str = "title_left_person_right";

pub(crate) static TEMPLATES: &[CoverTemplate] = &[
    CoverTemplate {
        id: HERO_CENTER,
        name: tr::editor::cover_templates::hero_center,
        layers: &[layer(
            (0.5, 0.5, 0.84, 0.34),
            (0.5, 0.45, 0.88, 0.3),
            Slot::Title(WHITE_OUTLINE),
        )],
    },
    CoverTemplate {
        id: TITLE_LEFT_PERSON_RIGHT,
        name: tr::editor::cover_templates::title_left_person_right,
        layers: &[
            layer(
                (0.3, 0.45, 0.54, 0.36),
                (0.5, 0.22, 0.88, 0.22),
                Slot::Title(YELLOW_OUTLINE),
            ),
            layer(
                (0.76, 0.5, 0.0, 0.92),
                (0.5, 0.5, 0.0, 0.58),
                Slot::Person {
                    side: TitleCardSubjectSide::Right,
                    expression: Expression::Smile,
                },
            ),
        ],
    },
    CoverTemplate {
        id: "title_right_person_left",
        name: tr::editor::cover_templates::title_right_person_left,
        layers: &[
            layer(
                (0.7, 0.45, 0.54, 0.36),
                (0.5, 0.22, 0.88, 0.22),
                Slot::Title(WHITE_OUTLINE),
            ),
            layer(
                (0.24, 0.5, 0.0, 0.92),
                (0.5, 0.5, 0.0, 0.58),
                Slot::Person {
                    side: TitleCardSubjectSide::Left,
                    expression: Expression::Point,
                },
            ),
        ],
    },
    CoverTemplate {
        id: "big_number",
        name: tr::editor::cover_templates::big_number,
        layers: &[
            layer(
                (0.5, 0.34, 0.5, 0.4),
                (0.5, 0.3, 0.8, 0.24),
                Slot::Text("3 个", YELLOW_BAND),
            ),
            layer(
                (0.5, 0.74, 0.86, 0.2),
                (0.5, 0.62, 0.88, 0.16),
                Slot::Title(WHITE_OUTLINE),
            ),
        ],
    },
    CoverTemplate {
        id: "question",
        name: tr::editor::cover_templates::question,
        layers: &[
            layer(
                (0.32, 0.45, 0.58, 0.36),
                (0.5, 0.22, 0.88, 0.22),
                Slot::Title(RED_BAND),
            ),
            layer(
                (0.76, 0.5, 0.0, 0.9),
                (0.5, 0.5, 0.0, 0.56),
                Slot::Person {
                    side: TitleCardSubjectSide::Right,
                    expression: Expression::Confused,
                },
            ),
            layer(
                (0.62, 0.2, 0.0, 0.18),
                (0.82, 0.42, 0.0, 0.1),
                Slot::Sticker("red-question-mark"),
            ),
        ],
    },
];

pub(crate) fn template(id: &str) -> Option<&'static CoverTemplate> {
    TEMPLATES.iter().find(|template| template.id == id)
}

fn portrait(output: (u32, u32)) -> bool {
    f64::from(output.1) > f64::from(output.0) * 1.15
}

fn pose((x, y, w, h): Box4) -> LayerPose {
    LayerPose {
        center: XY::new(x, y),
        size: XY::new(w, h),
        rotation: 0.0,
        opacity: 1.0,
    }
}

fn text_kind(style: Style, content: Option<&str>) -> CoverLayerKind {
    let (color, stroke, band) = (
        style.color.to_owned(),
        style.stroke.map(str::to_owned),
        style.band.map(str::to_owned),
    );
    match content {
        Some(content) => CoverLayerKind::Text {
            content: content.to_owned(),
            color,
            stroke,
            band,
            font: None,
        },
        None => CoverLayerKind::Title {
            color,
            stroke,
            band,
            font: None,
        },
    }
}

/// A picture of `source` pixels `height` tall, centred on `x` and standing
/// on the bottom edge (or centred on `y` when `stand` is false).
fn picture_box(
    x: f64,
    y: f64,
    height: f64,
    source: (u32, u32),
    output: (u32, u32),
    stand: bool,
) -> LayerPose {
    let aspect = f64::from(source.0.max(1)) / f64::from(source.1.max(1));
    let card = f64::from(output.0.max(1)) / f64::from(output.1.max(1));
    let width = (height * aspect / card).min(0.95);
    let height = width * card / aspect;
    let x = x.clamp(width / 2., 1. - width / 2.);
    let y = if stand { 1. - height / 2. } else { y };
    pose((x, y, width, height))
}

/// A finished layer list for a template: the files a person or sticker
/// needs are written into the project first. Runs off the main thread.
fn build_layers(
    project: &std::path::Path,
    template: &CoverTemplate,
    persona: Option<&Persona>,
    output: (u32, u32),
) -> Result<(Vec<CoverLayer>, Vec<crate::asset_library::AssetRecord>), String> {
    let tall = portrait(output);
    let skip_person = template.has_person() && persona.is_none();
    let mut layers = Vec::new();
    let mut records = Vec::new();
    for item in template.layers {
        let place = if tall { item.tall } else { item.wide };
        match item.slot {
            Slot::Title(style) => {
                // Without its person the title takes the middle.
                let place = if skip_person {
                    if tall {
                        TEMPLATES[0].layers[0].tall
                    } else {
                        TEMPLATES[0].layers[0].wide
                    }
                } else {
                    place
                };
                layers.push(CoverLayer::new(pose(place), text_kind(style, None)));
            }
            Slot::Text(content, style) => {
                layers.push(CoverLayer::new(
                    pose(place),
                    text_kind(style, Some(content)),
                ));
            }
            Slot::Person {
                side, expression, ..
            } => {
                let Some(persona) = persona else {
                    continue;
                };
                let poses = &persona.manifest.poses;
                let Some(chosen) = poses
                    .iter()
                    .find(|pose| pose.expression == expression)
                    .or_else(|| poses.first())
                else {
                    continue;
                };
                let (record, size) = personas::place_in_project(project, persona, chosen)?;
                let side = if tall {
                    TitleCardSubjectSide::Right
                } else {
                    side
                };
                layers.push(CoverLayer::new(
                    picture_box(place.0, place.1, place.3, size, output, true),
                    CoverLayerKind::Image {
                        path: record.path.clone(),
                        mirror: personas::mirror_for(chosen.facing, side),
                        persona: Some(CoverPersonaRef {
                            persona: persona.id.clone(),
                            file: chosen.file.clone(),
                        }),
                    },
                ));
                records.push(record);
            }
            Slot::Sticker(id) => {
                let Some(sticker) = crate::stickers::STICKERS
                    .emoji
                    .iter()
                    .chain(crate::stickers::STICKERS.icons.iter())
                    .find(|sticker| sticker.id == id)
                else {
                    continue;
                };
                let png = crate::stickers::write_png(sticker, "#EF4444", 2.)?;
                let bytes = std::fs::read(&png).map_err(|error| error.to_string())?;
                let (record, size) =
                    personas::save_layer_image(project, bytes, sticker.name.clone())?;
                layers.push(CoverLayer::new(
                    picture_box(place.0, place.1, place.3, size, output, false),
                    CoverLayerKind::Image {
                        path: record.path.clone(),
                        mirror: false,
                        persona: None,
                    },
                ));
                records.push(record);
            }
        }
    }
    Ok((layers, records))
}

impl EditorWindow {
    /// Replaces the card's layers with a template's. The title's words stay.
    pub(crate) fn apply_cover_template(
        &mut self,
        index: usize,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(template) = template(id) else {
            return;
        };
        let Some(card_id) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.as_ref())
            .map(|card| card.id.clone())
        else {
            return;
        };
        if self.sidebar.title_cards.layers_busy() {
            return;
        }
        let personas = self.sidebar.title_cards.subject.persona_list();
        let persona = template
            .has_person()
            .then(|| CoverPersona::load().persona(&personas))
            .flatten();
        let output = self.cover_output();
        let project_path = self.project_path.clone();
        self.sidebar.title_cards.set_layers_busy(true);
        self.sidebar.title_cards.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let built = cx
                .background_executor()
                .spawn(
                    async move { build_layers(&project_path, template, persona.as_ref(), output) },
                )
                .await;
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.set_layers_busy(false);
                match built {
                    Ok((layers, records)) => {
                        for record in records {
                            this.remember_project_asset(record, cx);
                        }
                        let Some(index) = this.card_index_by_id(&card_id) else {
                            return;
                        };
                        this.canvas_selection = None;
                        this.update_title_card(index, window, cx, move |segment| {
                            let card = segment.title_card.as_mut().unwrap();
                            card.layers = layers;
                            card.kicker.clear();
                            card.subtitle.clear();
                            card.subject = None;
                            card.custom_text_layout = false;
                            card.hide_title = false;
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

    /// The cover is still as a new card starts: only its centred title.
    pub(crate) fn cover_untouched(&self, index: usize) -> bool {
        let Some(card) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.as_ref())
        else {
            return false;
        };
        let output = self.cover_output();
        let start = if portrait(output) {
            TEMPLATES[0].layers[0].tall
        } else {
            TEMPLATES[0].layers[0].wide
        };
        matches!(card.layers.as_slice(), [only]
            if matches!(only.kind, CoverLayerKind::Title { .. }) && only.pose == pose(start))
            || card.layers.is_empty()
    }
}

/// The card's pixel shape for a project: tall for portrait ratios.
pub(crate) fn project_output(project: &cap_project::ProjectConfiguration) -> (u32, u32) {
    match project.aspect_ratio {
        Some(cap_project::AspectRatio::Vertical) => (1080, 1920),
        Some(cap_project::AspectRatio::Tall) => (1080, 1440),
        Some(cap_project::AspectRatio::Square) => (1080, 1080),
        _ => (1920, 1080),
    }
}

/// The layers a new card starts with: its title in the middle.
pub(crate) fn starting_layers(output: (u32, u32)) -> Vec<CoverLayer> {
    let item = TEMPLATES[0].layers[0];
    let place = if portrait(output) {
        item.tall
    } else {
        item.wide
    };
    let Slot::Title(style) = item.slot else {
        return Vec::new();
    };
    vec![CoverLayer::new(pose(place), text_kind(style, None))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_one_title_and_unique_ids() {
        let mut ids = std::collections::HashSet::new();
        for template in TEMPLATES {
            assert!(ids.insert(template.id));
            assert_eq!(
                template
                    .layers
                    .iter()
                    .filter(|layer| matches!(layer.slot, Slot::Title(_)))
                    .count(),
                1,
                "{}",
                template.id
            );
        }
        assert_eq!(starting_layers((1920, 1080)).len(), 1);
    }

    #[test]
    fn a_standing_picture_keeps_its_shape_and_touches_the_bottom() {
        let placed = picture_box(0.76, 0.5, 0.92, (600, 1000), (1920, 1080), true);
        let pixels = (placed.size.x * 1920., placed.size.y * 1080.);
        assert!((pixels.0 / pixels.1 - 0.6).abs() < 1e-9);
        assert!((placed.center.y + placed.size.y / 2. - 1.).abs() < 1e-9);
        assert!(placed.center.x + placed.size.x / 2. <= 1. + 1e-9);
    }
}
