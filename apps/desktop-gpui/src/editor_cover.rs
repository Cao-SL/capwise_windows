use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use cap_project::{ProjectConfiguration, TitleCardBackground, TitleCardImageFit, XY};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    Window, div, img, prelude::FluentBuilder, px,
};

use crate::{
    ai_store::{self, Kind},
    asset_library::{self, AssetKind, AssetRecord, PendingGeneratedSource},
    cover_generation::{
        self, ArkBackgroundClient, BackgroundRequest, Dimensions, GeneratedBackground,
        GeneratedOrigin, GenerationContext, GenerationError, GenerationScope, TextArea,
    },
    editor_timeline::TrackKind,
    editor_window::EditorWindow,
    i18n::tr,
    settings_cover, ui,
};

#[derive(Default)]
pub(crate) struct CoverUi {
    open: bool,
    target: Option<String>,
    input: Option<Entity<ui::TextInputState>>,
    _input_observer: Option<gpui::Subscription>,
    scope: GenerationScope,
    generation: u64,
    context: Option<GenerationContext>,
    client: Option<Arc<ArkBackgroundClient>>,
    task: Option<gpui::Task<()>>,
    busy: bool,
    applying: bool,
    result: Option<Arc<GeneratedBackground>>,
    preview: Option<Arc<RenderImage>>,
    /// Everything one request returned, for the user to pick from. Picking
    /// moves one into `result`/`preview` and applies it.
    candidates: Vec<(Arc<GeneratedBackground>, Arc<RenderImage>)>,
    /// The request painted the title (and the person) into the image, so the
    /// card stops drawing its own.
    finished: bool,
    /// Candidates still on their way, and when the request started.
    pending: usize,
    started: Option<std::time::Instant>,
    ticker: Option<gpui::Task<()>>,
    /// The last applied image, kept for the assistant's result card after the
    /// generation state is cleared.
    shown: Option<Arc<RenderImage>>,
    save_pending: Option<String>,
    error: Option<String>,
    saved: bool,
}

fn context_for(
    path: &Path,
    project: &ProjectConfiguration,
    target: &str,
) -> Option<GenerationContext> {
    project
        .timeline
        .as_ref()?
        .text_segments
        .iter()
        .find(|segment| {
            segment
                .title_card
                .as_ref()
                .is_some_and(|card| card.id == target)
        })?;
    // Layers placed over a cover, and where the person was dragged, never
    // reach the image request, so moving them while it runs keeps the result.
    let mut unplaced = project.clone();
    for card in unplaced
        .timeline
        .iter_mut()
        .flat_map(|timeline| timeline.text_segments.iter_mut())
        .filter_map(|segment| segment.title_card.as_mut())
    {
        card.layers.clear();
        if let Some(subject) = card.subject.as_mut() {
            subject.pose = None;
        }
    }
    let mut hash = DefaultHasher::new();
    serde_json::to_vec(&unplaced).ok()?.hash(&mut hash);
    Some(GenerationContext {
        project_id: path.to_string_lossy().into_owned(),
        target_id: target.into(),
        revision: hash.finish(),
    })
}

fn error_message(error: GenerationError) -> String {
    match error {
        GenerationError::Authentication
        | GenerationError::Permission
        | GenerationError::InvalidCredential => tr::editor::cover::authentication().into(),
        GenerationError::InvalidDescription => tr::editor::cover::description_required().into(),
        GenerationError::InvalidModel => tr::settings::cover::invalid_model().into(),
        GenerationError::UnsupportedDimensions => tr::editor::cover::unsupported_size().into(),
        GenerationError::Cancelled => tr::editor::cover::changed().into(),
        GenerationError::Timeout => tr::editor::cover::timeout().into(),
        GenerationError::RateLimited { .. } => tr::editor::cover::rate_limited().into(),
        GenerationError::ModelNotActivated => tr::editor::cover::model_not_activated().into(),
        GenerationError::HttpStatus(status) => tr::editor::cover::http_error(status),
        GenerationError::Rejected => tr::editor::cover::rejected().into(),
        GenerationError::Network => tr::editor::cover::network_error().into(),
        _ => tr::editor::cover::invalid_image().into(),
    }
}

fn thumbnail(bytes: &[u8]) -> Result<Arc<RenderImage>, GenerationError> {
    let mut rgba = image::load_from_memory(bytes)
        .map_err(|_| GenerationError::InvalidImage)?
        .thumbnail(640, 640)
        .to_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![
        image::Frame::new(rgba)
    ])))
}

struct PreparedBackground {
    root: PathBuf,
    directory: fs::Metadata,
    media: PathBuf,
    identity: fs::Metadata,
    record: AssetRecord,
    source: Option<PendingGeneratedSource>,
    keep: bool,
}

impl PreparedBackground {
    fn write(
        project: &Path,
        bytes: &[u8],
        extension: &str,
        origin: GeneratedOrigin,
    ) -> Result<Self, String> {
        if !matches!(extension, "png" | "jpg") || !origin.is_valid() {
            return Err(tr::editor::cover::invalid_image().into());
        }
        let root = project
            .canonicalize()
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let directory = fs::symlink_metadata(&root)
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let mut parent = root.clone();
        for child in ["content", "images"] {
            parent.push(child);
            match fs::create_dir(&parent) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(tr::editor::cover::save_failed().into()),
            }
            let metadata = fs::symlink_metadata(&parent)
                .map_err(|_| tr::editor::cover::save_failed().to_string())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(tr::editor::cover::save_failed().into());
            }
        }
        let name = format!("generated-{}.{}", uuid::Uuid::new_v4(), extension);
        let media = parent.join(&name);
        let mut file = fs::File::create_new(&media)
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let identity = file
            .metadata()
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        let mut staged = Self {
            root,
            directory,
            media,
            identity,
            record: AssetRecord {
                path: format!("content/images/{name}"),
                name: tr::editor::cover::asset_name().into(),
                kind: AssetKind::Photo,
                source: None,
                rendition: None,
            },
            source: None,
            keep: false,
        };
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| tr::editor::cover::save_failed().to_string())?;
        drop(file);
        staged.source = Some(PendingGeneratedSource::register(
            &staged.root,
            staged.record.clone(),
            origin,
        )?);
        Ok(staged)
    }

    fn retain(mut self) -> AssetRecord {
        if let Some(source) = self.source.take() {
            source.commit();
        }
        self.keep = true;
        self.record.clone()
    }
}

impl Drop for PreparedBackground {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        if let Some(mut source) = self.source.take()
            && source.rollback().is_err()
        {
            return;
        }
        if fs::symlink_metadata(&self.root)
            .is_ok_and(|current| asset_library::same_file_identity(&self.directory, &current))
            && fs::symlink_metadata(&self.media)
                .is_ok_and(|current| asset_library::same_file_identity(&self.identity, &current))
        {
            let _ = fs::remove_file(&self.media);
        }
    }
}

impl EditorWindow {
    fn cover_context(&self, target: &str) -> Option<GenerationContext> {
        if self.assistant.cover_active
            && self.sidebar.title_cards.cover.target.as_deref() == Some(target)
        {
            return context_for(&self.project_path, &self.project, target);
        }
        let index = self
            .selection
            .as_ref()
            .filter(|selection| selection.track == TrackKind::Text)?
            .indices
            .last()?;
        let selected = self
            .project
            .timeline
            .as_ref()?
            .text_segments
            .get(*index)?
            .title_card
            .as_ref()?;
        if selected.id != target {
            return None;
        }
        context_for(&self.project_path, &self.project, target)
    }

    pub(crate) fn open_assistant_cover(
        &mut self,
        description: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.project.timeline.as_ref().and_then(|timeline| {
            timeline.text_segments.iter().position(|segment| {
                segment
                    .title_card
                    .as_ref()
                    .is_some_and(|card| card.role == cap_project::TitleCardRole::Intro)
            })
        }) else {
            return;
        };
        // Show the cover in the player but keep the assistant on screen:
        // selecting the card would swap the sidebar to its settings, which
        // only a click on the result should do.
        if let Some(time) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| {
                crate::editor_title_cards::card_preview_time(
                    segment,
                    crate::editor_window::EDITOR_PREVIEW_FPS,
                )
            })
        {
            self.seek_to_time(time, cx);
        }
        self.open_cover_generator(index, window, cx);
        if let Some(input) = &self.sidebar.title_cards.cover.input {
            input.update(cx, |input, cx| input.set_text(description, cx));
        }
        cx.notify();
    }

    fn open_cover_generator(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(card) = self
            .project
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.text_segments.get(index))
            .and_then(|segment| segment.title_card.clone())
        else {
            return;
        };
        self.open_assistant(window, cx);
        self.assistant.cover_active = true;
        self.end_field_edit(cx);
        self.close_color_picker(cx);
        if self.sidebar.title_cards.cover.target.as_deref() != Some(&card.id) {
            self.sidebar.title_cards.cover = CoverUi::default();
        }
        if self.sidebar.title_cards.cover.input.is_none() {
            let input = cx.new(|cx| ui::TextInputState::multi_line(window, cx));
            input.update(cx, |input, _| {
                input.set_placeholder(tr::editor::cover::description_hint())
            });
            self.sidebar.title_cards.cover._input_observer =
                Some(cx.observe(&input, |_, _, cx| cx.notify()));
            self.sidebar.title_cards.cover.input = Some(input);
        }
        let state = &mut self.sidebar.title_cards.cover;
        state.target = Some(card.id);
        state.open = true;
        cx.notify();
    }

    pub(crate) fn cancel_cover_generation(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.sidebar.title_cards.cover;
        if state.applying {
            return;
        }
        state.scope.cancel();
        state.generation = state.generation.wrapping_add(1);
        state.task = None;
        state.busy = false;
        state.pending = 0;
        state.started = None;
        state.open = false;
        state.candidates.clear();
        if state.save_pending.is_none() {
            state.result = None;
            state.preview = None;
            state.context = None;
            state.error = None;
        }
        cx.notify();
    }

    pub(crate) fn generate_cover_background(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_cover_candidates(false, window, cx);
    }

    /// Two more candidates under the ones on offer.
    fn more_cover_candidates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_cover_candidates(true, window, cx);
    }

    fn request_cover_candidates(
        &mut self,
        more: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar.title_cards.cover.busy
            || self.sidebar.title_cards.cover.applying
            || self.sidebar.title_cards.cover.save_pending.is_some()
            || !self.project_ready()
        {
            return;
        }
        let Some((provider, endpoint)) = settings_cover::configured_endpoint() else {
            self.sidebar.title_cards.cover.error = Some(tr::editor::cover::configure().into());
            cx.notify();
            return;
        };
        let model = endpoint.model;
        let Some(target) = self.sidebar.title_cards.cover.target.clone() else {
            return;
        };
        let Some(context) = self.cover_context(&target) else {
            return;
        };
        let Some(instance) = &self.instance else {
            return;
        };
        let size = cap_rendering::ProjectUniforms::get_output_size(
            &instance.render_constants.options,
            &self.project,
            XY::new(1920, 1920),
        );
        let description = self
            .sidebar
            .title_cards
            .cover
            .input
            .as_ref()
            .map(|input| input.read(cx).text().to_string())
            .unwrap_or_default();
        if description.trim().is_empty() || description.len() > 8_000 {
            self.sidebar.title_cards.cover.error =
                Some(tr::editor::cover::description_required().into());
            cx.notify();
            return;
        }
        let mode = crate::cover_templates::CoverMode::load();
        let person = crate::cover_templates::CoverPersona::load()
            .persona(&self.sidebar.title_cards.subject.persona_list());
        let card = self.project.timeline.as_ref().and_then(|timeline| {
            timeline.text_segments.iter().find(|segment| {
                segment
                    .title_card
                    .as_ref()
                    .is_some_and(|card| card.id == target)
            })
        });
        // A collage asks only for a background and lays the title and the
        // person over it as layers; a painted cover has both drawn in, and
        // only then does a photo of the person leave the computer.
        let painted = mode == crate::cover_templates::CoverMode::Painted;
        let title = card
            .map(|segment| segment.content.trim().to_string())
            .filter(|title| painted && !title.is_empty() && title.chars().count() <= 80);
        let reference_path = person
            .as_ref()
            .filter(|_| title.is_some())
            .and_then(|persona| {
                let poses = &persona.manifest.poses;
                let pose = poses
                    .iter()
                    .find(|pose| pose.expression == crate::personas::Expression::Smile)
                    .or_else(|| poses.first())?;
                persona.pose_path(pose)
            });
        let text_area = if person.is_some() {
            TextArea::Left
        } else {
            TextArea::Center
        };
        let state = &mut self.sidebar.title_cards.cover;
        let client = match ArkBackgroundClient::for_endpoint(&endpoint.base_url, provider.is_ark())
            .map(Arc::new)
        {
            Ok(client) => client,
            Err(error) => {
                state.error = Some(error_message(error));
                cx.notify();
                return;
            }
        };
        state.client = Some(client.clone());
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        // More candidates keep the ones on offer pickable; a new request
        // replaces them.
        let more = more && !state.candidates.is_empty() && state.context.as_ref() == Some(&context);
        let ticket = match more.then(|| state.scope.more(context.clone())).flatten() {
            Some(ticket) => ticket,
            None => {
                state.candidates.clear();
                state.scope.begin(context.clone())
            }
        };
        state.context = Some(context.clone());
        state.busy = true;
        state.error = None;
        state.result = None;
        state.preview = None;
        state.finished = title.is_some();
        state.saved = false;
        let mut request = BackgroundRequest {
            profile: cover_generation::profile_for_model(&model),
            model,
            target: Dimensions {
                width: size.0,
                height: size.1,
            },
            description,
            text_area,
            title,
            reference: None,
        };
        state.pending = cover_generation::COVER_CANDIDATES;
        state.started = Some(std::time::Instant::now());
        // Tick the elapsed seconds while waiting.
        state.ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let waiting = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        this.sidebar.title_cards.cover.pending > 0
                    })
                    .unwrap_or(false);
                if !waiting {
                    return;
                }
            }
        }));
        state.task = Some(cx.spawn_in(window, async move |this, cx| {
            // The key and the reference photo once, then every candidate on
            // its own request, so each shows the moment it is ready.
            let Ok(setup) = cx.update(|_, cx| {
                gpui_tokio::Tokio::spawn(cx, async move {
                    let key = tokio::task::spawn_blocking(move || {
                        ai_store::key(Kind::Image, provider.id())
                    })
                    .await
                    .map_err(|_| GenerationError::InvalidCredential)?
                    .ok_or(GenerationError::InvalidCredential)?;
                    if let Some(path) = reference_path {
                        // A missing or unreadable photo only loses the
                        // likeness; the cover is still worth making.
                        request.reference = tokio::task::spawn_blocking(move || {
                            fs::read(&path)
                                .ok()
                                .and_then(|bytes| cover_generation::reference_png(&bytes).ok())
                        })
                        .await
                        .ok()
                        .flatten()
                        .map(Arc::new);
                    }
                    Ok::<_, GenerationError>((key, request))
                })
            }) else {
                return;
            };
            let setup = setup.await.unwrap_or(Err(GenerationError::Cancelled));
            let (key, request) = match setup {
                Ok(setup) => setup,
                Err(error) => {
                    this.update_in(cx, |this, window, cx| {
                        this.sidebar.title_cards.cover.pending = 1;
                        this.finish_cover_candidate(
                            generation,
                            &target,
                            &context,
                            Err(error),
                            window,
                            cx,
                        )
                    })
                    .ok();
                    return;
                }
            };
            let mut requests = futures_util::stream::FuturesUnordered::new();
            for _ in 0..cover_generation::COVER_CANDIDATES {
                let client = client.clone();
                let key = key.clone();
                let request = request.clone();
                let ticket = ticket.clone();
                let Ok(task) = cx.update(|_, cx| {
                    gpui_tokio::Tokio::spawn(cx, async move {
                        let started = std::time::Instant::now();
                        let mut results =
                            client.generate_candidates(&key, request, 1, ticket).await?;
                        tracing::info!(
                            seconds = started.elapsed().as_secs_f32(),
                            "cover candidate ready"
                        );
                        let result = results.pop().ok_or(GenerationError::InvalidResponse)?;
                        let permit = cover_generation::image_decode_permit().await?;
                        tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            let preview = thumbnail(&result.bytes)?;
                            Ok::<_, GenerationError>((Arc::new(result), preview))
                        })
                        .await
                        .map_err(|_| GenerationError::InvalidImage)?
                    })
                }) else {
                    return;
                };
                requests.push(task);
            }
            while let Some(result) = futures_util::StreamExt::next(&mut requests).await {
                let result = result.unwrap_or(Err(GenerationError::Cancelled));
                let keep_going = this
                    .update_in(cx, |this, window, cx| {
                        this.finish_cover_candidate(
                            generation, &target, &context, result, window, cx,
                        )
                    })
                    .unwrap_or(false);
                if !keep_going {
                    return;
                }
            }
        }));
        cx.notify();
    }

    /// One candidate came back (or failed). Returns whether to keep waiting
    /// for the rest.
    fn finish_cover_candidate(
        &mut self,
        generation: u64,
        target: &str,
        context: &GenerationContext,
        result: Result<(Arc<GeneratedBackground>, Arc<RenderImage>), GenerationError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.sidebar.title_cards.cover.generation != generation {
            return false;
        }
        let current = self.cover_context(target);
        let state = &mut self.sidebar.title_cards.cover;
        if current.as_ref() != Some(context) {
            state.scope.cancel();
            state.busy = false;
            state.pending = 0;
            state.task = None;
            state.error = Some(tr::editor::cover::changed().into());
            cx.notify();
            return false;
        }
        state.pending = state.pending.saturating_sub(1);
        match result {
            Ok((result, preview)) if state.scope.accepts(&result, context) => {
                state.candidates.push((result, preview));
            }
            Ok(_) => {}
            Err(GenerationError::Cancelled) => {}
            Err(error) => {
                if state.pending == 0 && state.candidates.is_empty() {
                    state.error = Some(error_message(error));
                }
                tracing::warn!(?error, "a cover candidate failed");
            }
        }
        // Setup failed: nothing else is coming.
        if state.candidates.is_empty() && state.error.is_some() {
            state.pending = 0;
        }
        let done = state.pending == 0;
        if done {
            state.busy = false;
            state.task = None;
            state.started = None;
            // `CAP_GPUI_AUTO_COVER_PICK=<n>`: pick candidate n the way a
            // click does (synthetic clicks are dropped).
            #[cfg(debug_assertions)]
            if let Some(index) = std::env::var("CAP_GPUI_AUTO_COVER_PICK")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
            {
                self.pick_cover_candidate(index, window, cx);
            }
        }
        cx.notify();
        window.refresh();
        !done
    }

    fn finish_cover_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.flush_pending_saves(cx) {
            Ok(()) => {
                let state = &mut self.sidebar.title_cards.cover;
                if let (Some(result), Some(context)) = (&state.result, &state.context) {
                    let _ = state.scope.acknowledge_applied(result, context);
                }
                state.save_pending = None;
                state.result = None;
                state.candidates.clear();
                state.shown = state.preview.take();
                state.saved = true;
                state.error = None;
            }
            Err(_) => {
                self.sidebar.title_cards.cover.error = Some(tr::editor::cover::save_failed().into())
            }
        }
        cx.notify();
        window.refresh();
    }

    fn apply_cover_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.title_cards.cover.busy
            || self.sidebar.title_cards.cover.applying
            || self.asset_import_busy()
            || self.voiceover.is_busy()
        {
            return;
        }
        let Some(target) = self.sidebar.title_cards.cover.target.clone() else {
            return;
        };
        if let Some(path) = self.sidebar.title_cards.cover.save_pending.clone() {
            let referenced = self.project.timeline.as_ref().is_some_and(|timeline| timeline.text_segments.iter().any(|segment| segment.title_card.as_ref().is_some_and(|card| card.id == target && matches!(&card.background, TitleCardBackground::ProjectImage { path: current, .. } if current == &path))));
            if referenced {
                self.finish_cover_save(window, cx);
            } else {
                let state = &mut self.sidebar.title_cards.cover;
                state.save_pending = None;
                state.result = None;
                state.preview = None;
                state.scope.cancel();
                self.sidebar.title_cards.cover.error = Some(tr::editor::cover::changed().into());
                cx.notify();
            }
            return;
        }
        let Some(context) = self.cover_context(&target) else {
            return;
        };
        let Some(result) = self.sidebar.title_cards.cover.result.clone() else {
            return;
        };
        if !self
            .sidebar
            .title_cards
            .cover
            .scope
            .accepts(&result, &context)
        {
            self.sidebar.title_cards.cover.error = Some(tr::editor::cover::changed().into());
            cx.notify();
            return;
        }
        self.sidebar.title_cards.cover.applying = true;
        self.sidebar.title_cards.cover.error = None;
        let finished = self.sidebar.title_cards.cover.finished;
        let project_path = self.project_path.clone();
        let generation = self.sidebar.title_cards.cover.generation;
        self.sidebar.title_cards.cover.task = Some(cx.spawn_in(window, async move |this, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move {
                    PreparedBackground::write(
                        &project_path,
                        &result.bytes,
                        result.extension,
                        result.origin.clone(),
                    )
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.sidebar.title_cards.cover.generation != generation {
                    return;
                }
                this.sidebar.title_cards.cover.applying = false;
                this.sidebar.title_cards.cover.task = None;
                if this.cover_context(&target).as_ref() != Some(&context) {
                    this.sidebar.title_cards.cover.error =
                        Some(tr::editor::cover::changed().into());
                    cx.notify();
                    return;
                }
                match prepared {
                    Ok(prepared) => {
                        let path = prepared.record.path.clone();
                        let changed = this.title_card_edit(
                            "title-card-ai-background",
                            window,
                            cx,
                            |project| {
                                let Some(segment) =
                                    project.timeline.as_mut().and_then(|timeline| {
                                        timeline.text_segments.iter_mut().find(|segment| {
                                            segment
                                                .title_card
                                                .as_ref()
                                                .is_some_and(|card| card.id == target)
                                        })
                                    })
                                else {
                                    return Err(cap_project::TitleCardError::MissingCard);
                                };
                                let card = segment.title_card.as_mut().unwrap();
                                card.background = TitleCardBackground::ProjectImage {
                                    path: path.clone(),
                                    // Fill the canvas: a model that returns a
                                    // near ratio crops a sliver instead of
                                    // leaving bars.
                                    fit: TitleCardImageFit::Cover,
                                    focus: XY::new(0.5, 0.5),
                                    // A finished cover is shown as made; a
                                    // plain background is dimmed under the
                                    // card's own title.
                                    dim: 0.0,
                                };
                                card.hide_title = finished;
                                if finished {
                                    // The picture already shows the title.
                                    card.layers.retain(|layer| {
                                        !matches!(
                                            layer.kind,
                                            cap_project::CoverLayerKind::Title { .. }
                                        )
                                    });
                                }
                                Ok(true)
                            },
                        );
                        if changed {
                            let record = prepared.retain();
                            // Every generated cover is worth keeping: put a
                            // copy in the shared library for later videos.
                            let keep = this.project_path.join(&path);
                            let name = this
                                .project
                                .title_card(cap_project::TitleCardRole::Intro)
                                .map(|segment| segment.content.clone())
                                .unwrap_or_default();
                            cx.background_executor()
                                .spawn(async move {
                                    if let Err(error) = crate::global_library::add(&keep, &name) {
                                        tracing::warn!(%error, "could not keep the cover in the library");
                                    }
                                })
                                .detach();
                            this.remember_project_asset(record, cx);
                            this.sidebar.title_cards.cover.save_pending = Some(path);
                            this.finish_cover_save(window, cx);
                            // A fresh card gets its title and person laid out
                            // over the new background; a cover already
                            // arranged by hand keeps its layers.
                            if !finished
                                && let Some(index) = this.card_index_by_id(&target)
                                && this.cover_untouched(index)
                            {
                                let template = if crate::cover_templates::CoverPersona::load()
                                    .persona(&this.sidebar.title_cards.subject.persona_list())
                                    .is_some()
                                {
                                    crate::cover_templates::TITLE_LEFT_PERSON_RIGHT
                                } else {
                                    crate::cover_templates::HERO_CENTER
                                };
                                this.apply_cover_template(index, template, window, cx);
                            }
                        }
                    }
                    Err(_) => {
                        this.sidebar.title_cards.cover.error =
                            Some(tr::editor::cover::save_failed().into())
                    }
                }
                cx.notify();
                window.refresh();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Use candidate `index` as the cover.
    fn pick_cover_candidate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.sidebar.title_cards.cover;
        if state.busy || state.applying || state.save_pending.is_some() {
            return;
        }
        let Some((result, preview)) = state.candidates.get(index).cloned() else {
            return;
        };
        state.result = Some(result);
        state.preview = Some(preview);
        self.apply_cover_background(window, cx);
    }

    /// Who goes on a generated cover and how it is made, remembered for
    /// every project.
    pub(crate) fn render_cover_ai_choices(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::cover_templates::{CoverMode, CoverPersona};
        let theme = self.theme;
        let persona = CoverPersona::load();
        let mode = CoverMode::load();
        let has_mine = CoverPersona::Mine
            .persona(&self.sidebar.title_cards.subject.persona_list())
            .is_some();
        let chip = |id: SharedString, label: &'static str, pressed: bool| {
            ui::EditorButton::plain(&theme, id)
                .choice(&theme)
                .height(px(26.))
                .text_size(px(12.))
                .label(label)
                .pressed(pressed)
        };
        let people =
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(CoverPersona::all().into_iter().map(|choice| {
                    chip(
                        SharedString::from(format!("cover-ai-person-{choice:?}")),
                        choice.label(),
                        persona == choice,
                    )
                    .disabled(choice == CoverPersona::Mine && !has_mine)
                    .when(choice == CoverPersona::Mine && !has_mine, |button| {
                        button.tooltip(&theme, tr::editor::cover_templates::mine_empty())
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        choice.save();
                        cx.notify();
                    }))
                }));
        let modes = div().flex().flex_wrap().gap(px(6.)).children(
            [
                (
                    CoverMode::Collage,
                    tr::editor::cover_templates::mode_collage(),
                ),
                (
                    CoverMode::Painted,
                    tr::editor::cover_templates::mode_painted(),
                ),
            ]
            .into_iter()
            .map(|(choice, label)| {
                chip(
                    SharedString::from(format!("cover-ai-mode-{choice:?}")),
                    label,
                    mode == choice,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    choice.save();
                    cx.notify();
                }))
            }),
        );
        let note = (mode == CoverMode::Painted).then(|| {
            let mut text = tr::editor::cover_templates::painted_note().to_string();
            if persona != CoverPersona::None {
                text.push(' ');
                text.push_str(tr::editor::title_cards::subject_privacy());
            }
            div()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(Hsla::from(theme.editor.text_3))
                .child(text)
        });
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(ui::Field::stacked(&theme, tr::editor::cover_templates::person()).child(people))
            .child(ui::Field::stacked(&theme, tr::editor::cover_templates::mode()).child(modes))
            .children(note)
            .into_any_element()
    }

    pub(crate) fn assistant_cover_busy(&self) -> bool {
        let state = &self.sidebar.title_cards.cover;
        state.busy || state.applying || state.save_pending.is_some()
    }

    /// The assistant's cover result: a placeholder while the image is made,
    /// then the image itself. No prompt, no form -- clicking the image opens
    /// the cover's own settings (title, duration) in the sidebar.
    pub(crate) fn render_assistant_cover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = self.theme;
        let state = &self.sidebar.title_cards.cover;
        let image = state.preview.clone().or_else(|| state.shown.clone());
        let busy = state.busy || state.applying;
        // The candidates, and the places still waiting for one.
        let picking = !state.applying && (!state.candidates.is_empty() || state.pending > 0);
        if !busy && !picking && image.is_none() && state.error.is_none() {
            return None;
        }
        let title = self
            .project
            .title_card(cap_project::TitleCardRole::Intro)
            .map(|segment| segment.content.clone())
            .unwrap_or_default();
        // Covers are shown whole, at their own ratio, the panel's width.
        let aspect = state
            .candidates
            .first()
            .map(|(result, _)| result.origin.actual)
            .map(|size| size.width as f32 / size.height.max(1) as f32)
            .or_else(|| {
                self.frame_layout.as_ref().map(|layout| {
                    layout.output_size[0] as f32 / layout.output_size[1].max(1) as f32
                })
            })
            .filter(|aspect| aspect.is_finite() && *aspect > 0.)
            .unwrap_or(16. / 9.);
        let cover_height = ((crate::editor_window::SIDEBAR_WIDTH - 36.) / aspect).clamp(120., 300.);
        let frame = div()
            .id("assistant-cover-image")
            .relative()
            .w_full()
            .h(px(168.))
            .rounded(px(10.))
            .overflow_hidden()
            .bg(Hsla::from(theme.editor.ctl));
        let frame = frame.h(px(cover_height));
        let frame = if picking {
            let waiting = state.busy && state.pending > 0;
            let heading = if waiting {
                tr::editor::assistant::cover_waiting(
                    state
                        .started
                        .map_or(0, |started| started.elapsed().as_secs())
                        as usize,
                )
                .to_string()
            } else {
                tr::editor::assistant::cover_pick().to_string()
            };
            let slot = |id: gpui::ElementId| {
                div()
                    .id(id)
                    .w_full()
                    .h(px(cover_height))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .bg(Hsla::from(theme.editor.ctl))
            };
            div()
                .id("assistant-cover-candidates")
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(heading),
                )
                .children(
                    state
                        .candidates
                        .iter()
                        .enumerate()
                        .map(|(index, (_, preview))| {
                            slot(("assistant-cover-candidate", index).into())
                                .cursor_pointer()
                                .border_1()
                                .border_color(Hsla::from(theme.editor.line))
                                .hover(|style| style.border_color(Hsla::from(theme.editor.accent)))
                                .when(!waiting, |slot| {
                                    slot.tooltip(move |_, cx| {
                                        ui::Tooltip::new(&theme, tr::editor::assistant::cover_use())
                                            .view(cx)
                                    })
                                })
                                .child(
                                    img(preview.clone())
                                        .size_full()
                                        .object_fit(gpui::ObjectFit::Contain),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick_cover_candidate(index, window, cx)
                                }))
                        }),
                )
                .children((0..if waiting { state.pending } else { 0 }).map(|index| {
                    slot(("assistant-cover-pending", index).into())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            ui::CircularProgress::new(
                                px(16.),
                                px(1.5),
                                Hsla::from(theme.editor.line),
                                Hsla::from(theme.editor.text_2),
                            )
                            .indeterminate(),
                        )
                }))
        } else {
            let frame = frame;
            match (&image, busy) {
                (Some(image), false) => frame
                    .cursor_pointer()
                    .child(
                        img(image.clone())
                            .size_full()
                            .rounded(px(10.))
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .tooltip(move |_, cx| {
                        ui::Tooltip::new(&theme, tr::editor::assistant::cover_settings()).view(cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_assistant(window, cx);
                        this.focus_title_card(cap_project::TitleCardRole::Intro, cx);
                        cx.notify();
                    })),
                _ => frame
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .when(busy, |frame| {
                        frame
                            .child(
                                ui::CircularProgress::new(
                                    px(16.),
                                    px(1.5),
                                    Hsla::from(theme.editor.line),
                                    Hsla::from(theme.editor.text_2),
                                )
                                .indeterminate(),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(Hsla::from(theme.editor.text_2))
                                    .child(tr::editor::assistant::cover_working()),
                            )
                    })
                    .when(!busy, |frame| {
                        frame.child(
                            gpui::svg()
                                .path("icons/image-off.svg")
                                .size(px(18.))
                                .text_color(Hsla::from(theme.editor.text_3)),
                        )
                    }),
            }
        };
        let mut card = div().flex().flex_col().gap(px(8.)).child(frame);
        if let Some(error) = &state.error {
            card = card.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(gpui::rgb(0xdc2626))
                    .child(error.clone()),
            );
        }
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(Hsla::from(theme.editor.text_2))
                        .child(title),
                )
                .when(busy, |row| {
                    row.child(
                        ui::EditorButton::plain(&theme, "assistant-cover-stop")
                            .left_icon("icons/square.svg")
                            .icon_size(px(12.))
                            .height(px(26.))
                            .tooltip(&theme, tr::editor::assistant::stop())
                            .disabled(state.applying)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_cover_generation(cx);
                            })),
                    )
                })
                .when(!busy && picking, |row| {
                    row.child(
                        ui::EditorButton::plain(&theme, "assistant-cover-more")
                            .left_icon("icons/plus.svg")
                            .icon_size(px(12.))
                            .height(px(26.))
                            .label(tr::editor::assistant::cover_more())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.more_cover_candidates(window, cx)
                            })),
                    )
                })
                .when(!busy && !picking, |row| {
                    row.child(
                        ui::EditorButton::plain(&theme, "assistant-cover-again")
                            .left_icon("icons/refresh-cw.svg")
                            .icon_size(px(13.))
                            .height(px(26.))
                            .tooltip(&theme, tr::editor::assistant::regenerate())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.generate_cover_background(window, cx)
                            })),
                    )
                    .when(image.is_some(), |row| {
                        row.child(
                            ui::EditorButton::plain(&theme, "assistant-cover-edit")
                                .left_icon("icons/sliders-horizontal.svg")
                                .icon_size(px(13.))
                                .height(px(26.))
                                .tooltip(&theme, tr::editor::assistant::cover_settings())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_assistant(window, cx);
                                    this.focus_title_card(cap_project::TitleCardRole::Intro, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            ui::EditorButton::plain(&theme, "assistant-cover-undo")
                                .left_icon("icons/undo.svg")
                                .icon_size(px(13.))
                                .height(px(26.))
                                .tooltip(&theme, tr::menu::edit::undo())
                                .disabled(!self.history.can_undo())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.undo(window, cx);
                                    let state = &mut this.sidebar.title_cards.cover;
                                    state.shown = None;
                                    state.preview = None;
                                    cx.notify();
                                })),
                        )
                    })
                }),
        );
        Some(card.into_any_element())
    }

    /// The title card panel's way in: one button that asks the assistant for a
    /// cover. The assistant writes the image brief and the image goes in.
    pub(crate) fn render_cover_generator(
        &self,
        _index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let busy = self.assistant_cover_busy() || self.assistant.busy;
        ui::Button::plain(
            &theme,
            "cover-ai-open",
            ui::ButtonVariant::Gray,
            ui::ButtonSize::Sm,
        )
        .label(if busy {
            tr::editor::cover::generating()
        } else {
            tr::editor::cover::open()
        })
        .disabled(busy || !self.project_ready())
        .on_click(cx.listener(|this, _, window, cx| this.request_assistant_cover(window, cx)))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover_generation::DimensionStatus;
    use cap_project::TitleCardRole;

    fn fixture() -> (Vec<u8>, GeneratedOrigin, ProjectConfiguration) {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            3,
            image::Rgb([15, 30, 120]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let dimensions = Dimensions {
            width: 4,
            height: 3,
        };
        let origin = GeneratedOrigin {
            version: 1,
            provider: "volcengineArk".into(),
            requested_model: "synthetic-model".into(),
            reported_model: None,
            target: dimensions,
            requested: dimensions,
            actual: dimensions,
            dimension_status: DimensionStatus::RequestedSize,
            created: None,
        };
        let mut project: ProjectConfiguration = serde_json::from_value(serde_json::json!({"timeline":{"segments":[{"start":0.0,"end":5.0,"timescale":1.0}],"zoomSegments":[]}})).unwrap();
        project
            .upsert_title_card(cap_project::TextSegment::new_title_card(
                TitleCardRole::Intro,
                "Synthetic cover",
            ))
            .unwrap();
        (bytes.into_inner(), origin, project)
    }

    #[test]
    fn cover_context_changes_with_card_project_and_manual_edit() {
        let (_, _, project) = fixture();
        let target = project
            .title_card(TitleCardRole::Intro)
            .unwrap()
            .title_card
            .as_ref()
            .unwrap()
            .id
            .clone();
        let initial = context_for(Path::new("synthetic.cap"), &project, &target).unwrap();
        let other = context_for(Path::new("other.cap"), &project, &target).unwrap();
        assert_ne!(initial.project_id, other.project_id);
        assert!(context_for(Path::new("synthetic.cap"), &project, "removed-card").is_none());
        let mut edited = project.clone();
        edited.timeline.as_mut().unwrap().text_segments[0].content = "Edited locally".into();
        assert_ne!(
            initial.revision,
            context_for(Path::new("synthetic.cap"), &edited, &target)
                .unwrap()
                .revision
        );
        let mut layered = project.clone();
        layered.timeline.as_mut().unwrap().text_segments[0]
            .title_card
            .as_mut()
            .unwrap()
            .layers
            .push(cap_project::CoverLayer::new(
                cap_project::LayerPose {
                    center: XY::new(0.5, 0.5),
                    size: XY::new(0.2, 0.1),
                    rotation: 0.0,
                    opacity: 1.0,
                },
                cap_project::CoverLayerKind::Text {
                    content: "新".into(),
                    color: "#FFFFFF".into(),
                    stroke: None,
                    band: None,
                    font: None,
                },
            ));
        assert_eq!(
            initial.revision,
            context_for(Path::new("synthetic.cap"), &layered, &target)
                .unwrap()
                .revision
        );
    }

    #[test]
    fn unaccepted_cover_staging_rolls_back_only_its_own_files() {
        let directory = tempfile::tempdir().unwrap();
        let (bytes, origin, _) = fixture();
        let first =
            PreparedBackground::write(directory.path(), &bytes, "png", origin.clone()).unwrap();
        let rejected_path = first.media.clone();
        let second = PreparedBackground::write(directory.path(), &bytes, "png", origin)
            .unwrap()
            .retain();
        drop(first);
        assert!(!rejected_path.exists());
        assert!(!asset_library::record_path(&rejected_path).exists());
        assert!(directory.path().join(&second.path).is_file());
        assert!(
            asset_library::read_generated_origin(directory.path(), &second.path)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn retained_cover_survives_disk_save_failure_retry_and_one_undo() {
        let directory = tempfile::tempdir().unwrap();
        let (bytes, origin, mut project) = fixture();
        let before = project.clone();
        let mut history = crate::editor_edits::ProjectHistory::new(before.clone());
        let record = PreparedBackground::write(directory.path(), &bytes, "png", origin.clone())
            .unwrap()
            .retain();
        project.timeline.as_mut().unwrap().text_segments[0]
            .title_card
            .as_mut()
            .unwrap()
            .background = TitleCardBackground::ProjectImage {
            path: record.path.clone(),
            fit: TitleCardImageFit::Contain,
            focus: XY::new(0.5, 0.5),
            dim: 0.25,
        };
        history.record(&project);
        let mut pending = crate::project_save::PendingProjectSave::new(directory.path().into());
        pending.queue(project.clone());
        fs::create_dir(directory.path().join("project-config.json")).unwrap();
        assert!(pending.try_flush().is_err());
        assert_eq!(
            fs::read(directory.path().join(&record.path)).unwrap(),
            bytes
        );
        fs::remove_dir(directory.path().join("project-config.json")).unwrap();
        pending.try_flush().unwrap();
        let reopened: ProjectConfiguration = serde_json::from_slice(
            &fs::read(directory.path().join("project-config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&reopened).unwrap(),
            serde_json::to_value(&project).unwrap()
        );
        assert_eq!(
            asset_library::read_generated_origin(directory.path(), &record.path).unwrap(),
            Some(origin)
        );
        assert_eq!(
            serde_json::to_value(history.undo().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(history.undo().is_none());
        assert_eq!(
            serde_json::to_value(history.redo().unwrap()).unwrap(),
            serde_json::to_value(project).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn generated_cover_refuses_symlinked_content_directory() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.path().join("content")).unwrap();
        let (bytes, origin, _) = fixture();
        assert!(PreparedBackground::write(directory.path(), &bytes, "png", origin).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
