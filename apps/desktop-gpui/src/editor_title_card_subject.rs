//! The user's own persona: photos they upload are cut out locally and kept
//! as poses beside the bundled personas, for covers.

use std::path::PathBuf;

use gpui::{AsyncWindowContext, Context, Window};

use crate::{
    editor_window::EditorWindow,
    i18n::tr,
    personas::{self, Persona},
};

#[derive(Default)]
pub(crate) struct SubjectUi {
    personas: std::cell::OnceCell<Vec<Persona>>,
    pub(crate) busy: bool,
}

impl SubjectUi {
    fn personas(&self) -> &[Persona] {
        self.personas.get_or_init(personas::all)
    }

    pub(crate) fn persona_list(&self) -> Vec<Persona> {
        self.personas().to_vec()
    }

    pub(crate) fn persona_ids(&self) -> Vec<String> {
        self.personas()
            .iter()
            .map(|persona| persona.id.clone())
            .collect()
    }
}

impl EditorWindow {
    pub(crate) fn import_persona_photo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                            this.sidebar
                                .title_cards
                                .layers
                                .show_persona(personas::PRIVATE_ID);
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
}
