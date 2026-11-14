//! The intro card's person: pick a pose from a persona (a bundled one or the
//! user's own photos) and it stands beside the title.

use std::path::PathBuf;

use cap_project::TitleCardSubject;
use gpui::{
    AnyElement, AsyncWindowContext, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder, px,
};

use crate::{
    editor_window::EditorWindow,
    i18n::tr,
    personas::{self, Persona},
    ui,
};

#[derive(Default)]
pub(crate) struct SubjectUi {
    personas: std::cell::OnceCell<Vec<Persona>>,
    browsing: Option<String>,
    placed: Option<(String, String)>,
    busy: bool,
}

impl SubjectUi {
    fn personas(&self) -> &[Persona] {
        self.personas.get_or_init(personas::all)
    }

    /// The clean pose behind the placed cutout (no sticker outline), when
    /// this session placed it -- the better reference for a generated cover.
    pub(crate) fn placed_source(&self) -> Option<std::path::PathBuf> {
        let (persona_id, file) = self.placed.as_ref()?;
        let persona = self
            .personas()
            .iter()
            .find(|persona| &persona.id == persona_id)?;
        let pose = persona
            .manifest
            .poses
            .iter()
            .find(|pose| &pose.file == file)?;
        persona.pose_path(pose)
    }
}

impl EditorWindow {
    fn place_persona_pose(
        &mut self,
        index: usize,
        persona_id: String,
        file: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar.title_cards.subject.busy {
            return;
        }
        let Some(persona) = self
            .sidebar
            .title_cards
            .subject
            .personas()
            .iter()
            .find(|persona| persona.id == persona_id)
            .cloned()
        else {
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
        let side = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.as_ref())
            .and_then(|card| card.subject.as_ref())
            .map(|subject| subject.side)
            .unwrap_or_else(|| personas::default_side(pose.facing));
        let project_path = self.project_path.clone();
        self.sidebar.title_cards.subject.busy = true;
        self.sidebar.title_cards.error = None;
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let facing = pose.facing;
            let placed = cx
                .background_executor()
                .spawn({
                    let persona = persona.clone();
                    async move { personas::place_in_project(&project_path, &persona, &pose) }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.sidebar.title_cards.subject.busy = false;
                match placed {
                    Ok(record) => {
                        this.remember_project_asset(record.clone(), cx);
                        let current = this.project.timeline.as_ref().and_then(|timeline| {
                            timeline.text_segments.iter().position(|segment| {
                                segment
                                    .title_card
                                    .as_ref()
                                    .is_some_and(|card| card.id == card_id)
                            })
                        });
                        let Some(index) = current else {
                            this.title_card_error(
                                tr::editor::title_cards::changed().into(),
                                window,
                                cx,
                            );
                            return;
                        };
                        this.sidebar.title_cards.subject.placed = Some((persona.id.clone(), file));
                        this.update_title_card(index, window, cx, move |segment| {
                            segment.title_card.as_mut().unwrap().subject = Some(TitleCardSubject {
                                path: record.path,
                                side,
                                mirror: personas::mirror_for(facing, side),
                            });
                        });
                    }
                    Err(error) => this.title_card_error(error, window, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_subject_side(
        &mut self,
        index: usize,
        side: cap_project::TitleCardSubjectSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_title_card(index, window, cx, move |segment| {
            if let Some(subject) = segment.title_card.as_mut().unwrap().subject.as_mut()
                && subject.side != side
            {
                subject.side = side;
                subject.mirror = !subject.mirror;
            }
        });
    }

    fn clear_subject(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.title_cards.subject.placed = None;
        self.update_title_card(index, window, cx, |segment| {
            segment.title_card.as_mut().unwrap().subject = None;
        });
    }

    fn import_persona_photo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.title_cards.subject.busy {
            return;
        }
        self.sidebar.title_cards.subject.busy = true;
        self.sidebar.title_cards.error = None;
        cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
            let filters: [(&str, &[&str]); 1] = [(
                tr::editor::window::image::filter(),
                &["png", "jpg", "jpeg", "webp", "heic"],
            )];
            #[cfg(target_os = "linux")]
            let source = crate::platform::open_file_panel_async(&filters, None, cx).await;
            #[cfg(not(target_os = "linux"))]
            let source = crate::import::pick_import_file(&filters, cx).await;
            let Some(source): Option<PathBuf> = source else {
                this.update(cx, |this, cx| {
                    this.sidebar.title_cards.subject.busy = false;
                    cx.notify();
                })
                .ok();
                return;
            };
            let name = tr::editor::title_cards::subject_mine().to_string();
            let imported = cx
                .background_executor()
                .spawn(async move { personas::import_photos(&[source], &name) })
                .await;
            this.update_in(cx, |this, window, cx| {
                let state = &mut this.sidebar.title_cards.subject;
                state.busy = false;
                state.personas = std::cell::OnceCell::from(personas::all());
                match imported {
                    Ok(summary) => {
                        if summary.poses > 0 {
                            state.browsing = Some(personas::PRIVATE_ID.into());
                            let message = tr::editor::title_cards::subject_imported(summary.poses)
                                .to_string();
                            this.show_notice(message, window, cx);
                        }
                        if summary.uncut > 0 {
                            let message =
                                tr::editor::title_cards::subject_kept(summary.uncut).to_string();
                            this.title_card_error(message, window, cx);
                        }
                    }
                    Err(error) => this.title_card_error(error, window, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_title_card_subject(
        &self,
        index: usize,
        subject: Option<&TitleCardSubject>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let busy = self.sidebar.title_cards.subject.busy || self.asset_import_busy();
        let personas = self.sidebar.title_cards.subject.personas().to_vec();
        let browsing = self
            .sidebar
            .title_cards
            .subject
            .browsing
            .clone()
            .or_else(|| personas.first().map(|persona| persona.id.clone()));
        let placed = self.sidebar.title_cards.subject.placed.clone();
        let chip = |id: String, label: String, pressed: bool| {
            ui::EditorButton::plain(&theme, gpui::ElementId::Name(id.into()))
                .choice(&theme)
                .height(px(28.))
                .text_size(px(12.))
                .label(label)
                .pressed(pressed)
        };
        let mut chips = div().flex().flex_wrap().gap(px(6.)).child(
            chip(
                "subject-none".into(),
                tr::editor::title_cards::subject_none().into(),
                subject.is_none(),
            )
            .disabled(busy || subject.is_none())
            .on_click(
                cx.listener(move |this, _, window, cx| this.clear_subject(index, window, cx)),
            ),
        );
        for persona in &personas {
            let id = persona.id.clone();
            let name = if persona.builtin {
                persona.display_name().to_owned()
            } else {
                tr::editor::title_cards::subject_mine().to_string()
            };
            chips = chips.child(
                chip(
                    format!("subject-persona-{id}"),
                    name,
                    browsing.as_deref() == Some(id.as_str()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sidebar.title_cards.subject.browsing = Some(id.clone());
                    cx.notify();
                })),
            );
        }
        chips = chips.child(
            ui::EditorButton::plain(&theme, "subject-upload")
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
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.import_persona_photo(window, cx))),
        );

        let grid = browsing
            .as_ref()
            .and_then(|id| personas.iter().find(|persona| &persona.id == id))
            .map(|persona| {
                div().grid().grid_cols(6).gap(px(4.)).children(
                    persona.manifest.poses.iter().filter_map(|pose| {
                        let path = persona.pose_path(pose)?;
                        let persona_id = persona.id.clone();
                        let file = pose.file.clone();
                        let selected =
                            placed
                                .as_ref()
                                .is_some_and(|(placed_persona, placed_file)| {
                                    placed_persona == &persona_id && placed_file == &file
                                })
                                && subject.is_some();
                        Some(
                            div()
                                .id(gpui::ElementId::Name(
                                    format!("subject-pose-{persona_id}-{file}").into(),
                                ))
                                .h(px(52.))
                                .rounded(px(6.))
                                .overflow_hidden()
                                .bg(Hsla::from(theme.editor.card))
                                .border_1()
                                .border_color(if selected {
                                    Hsla::from(theme.editor.accent)
                                } else {
                                    Hsla::from(theme.editor.line)
                                })
                                .when(!busy, |this| {
                                    this.cursor_pointer().hover(|this| {
                                        this.border_color(Hsla::from(theme.editor.accent))
                                    })
                                })
                                .child(img(path).size_full().object_fit(gpui::ObjectFit::Contain))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.place_persona_pose(
                                        index,
                                        persona_id.clone(),
                                        file.clone(),
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    }),
                )
            });

        let side = subject.map(|subject| {
            let current = subject.side;
            ui::SegmentedControl::editor(
                &theme,
                "title-card-subject-side",
                [
                    (
                        cap_project::TitleCardSubjectSide::Left,
                        tr::editor::title_cards::subject_left(),
                    ),
                    (
                        cap_project::TitleCardSubjectSide::Right,
                        tr::editor::title_cards::subject_right(),
                    ),
                ]
                .into_iter()
                .map(|(value, label)| ui::SegmentOption::new(label, value == current))
                .collect(),
            )
            .stretch()
            .on_select(cx.listener(move |this, selected: &usize, window, cx| {
                let side = [
                    cap_project::TitleCardSubjectSide::Left,
                    cap_project::TitleCardSubjectSide::Right,
                ][*selected];
                this.set_subject_side(index, side, window, cx);
            }))
        });

        ui::Field::stacked(&theme, tr::editor::title_cards::subject())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(chips)
                    .children(grid)
                    .children(side)
                    .child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(Hsla::from(theme.editor.text_3))
                            .child(tr::editor::title_cards::subject_privacy()),
                    ),
            )
            .into_any_element()
    }
}
