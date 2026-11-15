use cap_project::{
    CoverLayerKind, TextAlign, TextBackgroundStyle, TextSegment, TitleCardBackground,
};

use super::{ImageLayer, TextLayer, text::TextBackgroundResources};
use crate::{
    RenderVideoConstants, RenderingError,
    text::{PreparedText, parse_color},
    title_cards::{
        TitleCardLayout, balanced_title_lines, segment_title_card_layout, stacked_bounds,
    },
};

pub(crate) struct TitleCardLayer {
    text: TextLayer,
    image: ImageLayer,
    subject: ImageLayer,
    layer_images: ImageLayer,
    background: TextBackgroundResources,
    prepared: Option<(TextSegment, (u32, u32), bool)>,
    text_box: Option<crate::TitleTextFrameBox>,
    accent: bool,
    /// Tracks 1..=layers carry the cover layers; the card's own texts are on
    /// track 0. Only set while there are layers.
    layer_tracks: u32,
    visible: bool,
}

impl TitleCardLayer {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            text: TextLayer::new(device, queue),
            image: ImageLayer::new(device),
            subject: ImageLayer::new(device),
            layer_images: ImageLayer::new(device),
            background: TextBackgroundResources::new(device),
            prepared: None,
            text_box: None,
            accent: false,
            layer_tracks: 0,
            visible: false,
        }
    }

    pub(crate) fn hide(&mut self) {
        self.visible = false;
    }

    pub(crate) fn text_frame_box(&self) -> Option<crate::TitleTextFrameBox> {
        self.visible.then_some(self.text_box).flatten()
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) async fn prepare(
        &mut self,
        constants: &RenderVideoConstants,
        output: (u32, u32),
        segment: &TextSegment,
        index: usize,
        editing: bool,
    ) -> Result<(), RenderingError> {
        self.visible = false;
        let card = segment
            .title_card
            .as_ref()
            .ok_or_else(|| RenderingError::TitleCard("Missing card design".into()))?;
        card.validate()
            .map_err(|error| RenderingError::TitleCard(error.to_string()))?;
        if !card.hide_title && card.layers.is_empty() && segment.content.trim().is_empty() {
            return Err(RenderingError::TitleCard(
                "Enter a title before exporting the card".into(),
            ));
        }
        if !segment.font_size.is_finite() {
            return Err(RenderingError::TitleCard("Invalid title size".into()));
        }
        let layout = segment_title_card_layout(output, segment)
            .ok_or_else(|| RenderingError::TitleCard("Invalid output size".into()))?;

        if let TitleCardBackground::ProjectImage {
            path,
            fit,
            focus,
            dim,
        } = &card.background
        {
            self.image
                .prepare_title_card_picture(
                    constants,
                    path,
                    layout.image.unwrap(),
                    *fit,
                    *focus,
                    *dim,
                )
                .await?;
        } else {
            self.image.clear_title_card_picture();
        }

        // A finished cover already shows the person; drawing the cutout too
        // would put them on it twice.
        match (
            card.subject.as_ref().filter(|_| !card.hide_title),
            layout.subject,
        ) {
            (Some(subject), Some((region, side))) => {
                self.subject
                    .prepare_title_card_subject(
                        constants,
                        &subject.path,
                        region,
                        side,
                        subject.mirror,
                        subject.pose.map(|pose| (pose, output)),
                    )
                    .await?;
            }
            _ => self.subject.clear_title_card_picture(),
        }

        let pictures: Vec<_> = card
            .layers
            .iter()
            .enumerate()
            .filter_map(|(z, layer)| match &layer.kind {
                CoverLayerKind::Image { path, mirror } => {
                    Some((z as u32 + 1, path.clone(), layer.pose, *mirror))
                }
                CoverLayerKind::Text { .. } => None,
            })
            .collect();
        if pictures.is_empty() {
            self.layer_images.clear_title_card_picture();
        } else {
            self.layer_images
                .prepare_cover_layers(constants, output, &pictures)
                .await;
        }

        if self
            .prepared
            .as_ref()
            .is_none_or(|(previous, size, was_editing)| {
                previous != segment || *size != output || *was_editing != editing
            })
        {
            let mut texts = if card.hide_title {
                Vec::new()
            } else {
                self.prepare_texts(segment, &layout)?
            };
            let layered = !card.layers.is_empty();
            if layered {
                for text in &mut texts {
                    text.track = 0;
                }
            }
            self.text_box = None;
            if let Some(title) = texts.get_mut(usize::from(!card.kicker.trim().is_empty())) {
                self.text_box = Some(crate::TitleTextFrameBox {
                    segment_index: index,
                    bounds: title.bounds,
                    font_size: title.font_size,
                    line_height: title.line_height,
                    align: title.align,
                });
                if editing {
                    title.opacity = 0.0;
                }
            }
            if layered {
                for (z, layer) in card.layers.iter().enumerate() {
                    if let Some(text) = self.cover_text(layer, z as u32 + 1, output) {
                        texts.push(text);
                    }
                }
                self.text.prepare_exact_segmented(
                    &constants.device,
                    &constants.queue,
                    output,
                    &texts,
                );
            } else {
                self.text
                    .prepare_exact(&constants.device, &constants.queue, output, &texts);
            }
            self.layer_tracks = card.layers.len() as u32;
            let color = match &card.background {
                TitleCardBackground::Solid { color } => parse_color(color),
                TitleCardBackground::ProjectImage { .. } => parse_color("#111827"),
            };
            let mut rectangles = vec![([0.0, 0.0, output.0 as f32, output.1 as f32], color)];
            let accent = layout.accent.filter(|_| !card.hide_title);
            self.accent = accent.is_some();
            if let Some(bounds) = accent {
                let mut color = parse_color(&segment.color);
                color[3] = if layout.secondary.is_some() {
                    0.22
                } else {
                    0.78
                };
                rectangles.push((bounds, color));
            }
            self.background.prepare_rectangles(
                &constants.device,
                &constants.queue,
                output,
                &rectangles,
            );
            self.prepared = Some((segment.clone(), output, editing));
        }
        if let Some(text_box) = &mut self.text_box {
            text_box.segment_index = index;
        }
        self.visible = true;
        Ok(())
    }

    fn prepare_texts(
        &mut self,
        segment: &TextSegment,
        layout: &TitleCardLayout,
    ) -> Result<Vec<PreparedText>, RenderingError> {
        let card = segment.title_card.as_ref().unwrap();
        let font_size = segment.font_size.clamp(24.0, 240.0) * layout.scale;
        let mut blocks = Vec::new();
        let height = layout.text[3] - layout.text[1];
        let kicker = !card.kicker.trim().is_empty();
        let subtitle = !card.subtitle.trim().is_empty();
        let stacked_subtitle = subtitle && layout.secondary.is_none();
        let gaps = usize::from(kicker) + usize::from(stacked_subtitle);
        let available = height - layout.gap * gaps as f32;
        let kicker_limit = if kicker { available * 0.14 } else { 0.0 };
        let subtitle_limit = if stacked_subtitle {
            available * 0.26
        } else {
            0.0
        };
        if kicker {
            let mut text = card_text(
                segment,
                &card.kicker,
                font_size * 0.21,
                layout.text,
                layout.align,
            );
            text.font_weight = 600.0;
            text.letter_spacing = text.font_size * 0.06;
            text.color[3] = 0.72;
            text.line_height = 1.35;
            blocks.push(self.fit_text(
                text,
                kicker_limit,
                12.0 * layout.scale,
                "Short label",
                false,
            )?);
        }
        let content = if segment.uppercase {
            segment.content.to_uppercase()
        } else {
            segment.content.clone()
        };
        let mut title = card_text(segment, &content, font_size, layout.text, layout.align);
        title.font_weight = segment.font_weight;
        title.italic = segment.italic;
        title.line_height = if segment.line_height.is_finite() {
            segment.line_height.clamp(1.0, 1.4)
        } else {
            1.16
        };
        title.letter_spacing = segment.letter_spacing.clamp(-8.0, 24.0) * layout.scale;
        title.shadow = segment.shadow.clamp(0.0, 1.0);
        title.glow = segment.glow.clamp(0.0, 1.0);
        title.opacity = segment.opacity.clamp(0.0, 1.0);
        title.stroke = (segment.stroke_width > 0.0).then(|| {
            (
                segment.stroke_width.clamp(0.0, 12.0) * layout.scale,
                parse_color(&segment.stroke_color),
            )
        });
        blocks.push(self.fit_text(
            title,
            available - kicker_limit - subtitle_limit,
            24.0 * layout.scale,
            "Title",
            true,
        )?);
        if stacked_subtitle {
            let mut text = card_text(
                segment,
                &card.subtitle,
                font_size * 0.34,
                layout.text,
                layout.align,
            );
            text.color[3] = 0.84;
            text.line_height = 1.45;
            blocks.push(self.fit_text(
                text,
                subtitle_limit,
                14.0 * layout.scale,
                "Subtitle",
                false,
            )?);
        }
        let heights: Vec<f32> = blocks
            .iter()
            .map(|text| text.bounds[3] - text.bounds[1])
            .collect();
        let bounds = stacked_bounds(layout.text, &heights, layout.gap).ok_or_else(|| {
            RenderingError::TitleCard("Shorten the title or subtitle to fit the card".into())
        })?;
        for (text, bounds) in blocks.iter_mut().zip(bounds) {
            text.bounds = bounds;
        }
        if let Some(bounds) = layout.secondary.filter(|_| subtitle) {
            let mut text = card_text(
                segment,
                &card.subtitle,
                font_size * 0.4,
                bounds,
                TextAlign::Left,
            );
            text.color[3] = 0.84;
            text.line_height = 1.45;
            let mut text = self.fit_text(
                text,
                bounds[3] - bounds[1],
                14.0 * layout.scale,
                "Subtitle",
                false,
            )?;
            let height = text.bounds[3] - text.bounds[1];
            text.bounds = stacked_bounds(bounds, &[height], 0.0).unwrap()[0];
            blocks.push(text);
        }
        Ok(blocks)
    }

    /// A text layer fills its box: the words are as large as the box is
    /// tall, smaller only when a line would not fit its width.
    fn cover_text(
        &mut self,
        layer: &cap_project::CoverLayer,
        track: u32,
        output: (u32, u32),
    ) -> Option<PreparedText> {
        let CoverLayerKind::Text {
            content,
            color,
            stroke,
            band,
        } = &layer.kind
        else {
            return None;
        };
        let content = content.trim();
        if content.is_empty() {
            return None;
        }
        let pose = &layer.pose;
        let (width, height) = (
            (pose.size.x * f64::from(output.0)) as f32,
            (pose.size.y * f64::from(output.1)) as f32,
        );
        let (center_x, center_y) = (
            (pose.center.x * f64::from(output.0)) as f32,
            (pose.center.y * f64::from(output.1)) as f32,
        );
        if ![width, height, center_x, center_y]
            .iter()
            .all(|value| value.is_finite())
            || width <= 1.0
            || height <= 1.0
        {
            return None;
        }
        let bounds = [
            center_x - width * 0.5,
            center_y - height * 0.5,
            center_x + width * 0.5,
            center_y + height * 0.5,
        ];
        let lines = content.lines().count().max(1) as f32;
        let line_height = 1.15;
        let mut text = PreparedText {
            track,
            content: content.to_owned(),
            bounds,
            color: parse_color(color),
            background_color: band.as_deref().map(parse_color),
            font_family: "Noto Sans SC".into(),
            font_size: (height / (lines * line_height)).clamp(4.0, 2000.0),
            font_weight: 900.0,
            italic: false,
            opacity: pose.opacity.clamp(0.0, 1.0),
            align: TextAlign::Center,
            letter_spacing: 0.0,
            line_height,
            shadow: 0.0,
            offset: [0.0; 2],
            scale: 1.0,
            stroke: None,
            glow: 0.0,
            gradient_color: None,
            background_style: TextBackgroundStyle::Box,
            wipe: 1.0,
            stagger_in: None,
            stagger_out: None,
        };
        let mut measured = self.text.measure_exact(&text);
        while (measured.0 > width + 0.01 || measured.1 > height + 0.01) && text.font_size > 4.0 {
            text.font_size = (text.font_size * 0.92).max(4.0);
            measured = self.text.measure_exact(&text);
        }
        text.stroke = stroke
            .as_deref()
            .map(|color| (text.font_size * 0.09, parse_color(color)));
        let top = center_y - measured.1 * 0.5;
        text.bounds = [bounds[0], top, bounds[2], top + measured.1];
        Some(text)
    }

    fn fit_text(
        &mut self,
        mut text: PreparedText,
        max_height: f32,
        minimum: f32,
        label: &str,
        balance_lines: bool,
    ) -> Result<PreparedText, RenderingError> {
        let minimum = minimum.min(text.font_size);
        let original_content = text.content.clone();
        loop {
            let balanced = if balance_lines {
                let mut probe = text.clone();
                balanced_title_lines(
                    &original_content,
                    text.bounds[2] - text.bounds[0],
                    text.font_size * text.line_height,
                    |content| {
                        probe.content = content.to_owned();
                        self.text.measure_exact(&probe)
                    },
                )
            } else {
                Some(original_content.clone())
            };
            let acceptable = balanced.is_some();
            text.content = balanced.unwrap_or_else(|| original_content.clone());
            let (width, height) = self.text.measure_exact(&text);
            if acceptable
                && width <= text.bounds[2] - text.bounds[0] + 0.01
                && height <= max_height + 0.01
            {
                text.bounds[3] = text.bounds[1] + height;
                return Ok(text);
            }
            if text.font_size <= minimum + 0.001 {
                return Err(RenderingError::TitleCard(format!(
                    "{label} is too long for this card. Shorten it or choose another template."
                )));
            }
            text.font_size = (text.font_size * 0.9).max(minimum);
        }
    }

    pub(crate) fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.visible {
            return;
        }
        self.background.render(pass, 0);
        self.image.render(pass);
        self.subject.render(pass);
        if self.accent {
            self.background.render(pass, 1);
        }
        if self.layer_tracks == 0 {
            self.text.render(pass);
            return;
        }
        self.text.render_track(pass, 0);
        for track in 1..=self.layer_tracks {
            self.layer_images.render_track(pass, track);
            self.text.render_track(pass, track);
        }
    }
}

fn card_text(
    segment: &TextSegment,
    content: &str,
    font_size: f32,
    bounds: [f32; 4],
    align: TextAlign,
) -> PreparedText {
    PreparedText {
        track: segment.track,
        content: content.trim().to_owned(),
        bounds,
        color: parse_color(&segment.color),
        background_color: None,
        font_family: segment.font_family.clone(),
        font_size,
        font_weight: 400.0,
        italic: false,
        opacity: 1.0,
        align,
        letter_spacing: 0.0,
        line_height: 1.2,
        shadow: 0.0,
        offset: [0.0; 2],
        scale: 1.0,
        stroke: None,
        glow: 0.0,
        gradient_color: None,
        background_style: TextBackgroundStyle::Box,
        wipe: 1.0,
        stagger_in: None,
        stagger_out: None,
    }
}
