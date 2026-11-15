use super::*;
use crate::{TimelineFrameMapping, synchronize_captions};
use serde_json::{Value, json};

fn value(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn fixture() -> ProjectConfiguration {
    serde_json::from_value(json!({
        "textSizeVersion": 1,
        "textAnimVersion": 1,
        "timeline": {
            "segments": [{"recordingSegment": 0, "start": 0.0, "end": 10.0, "timescale": 1.0}],
            "zoomSegments": [{"start": 1.0, "end": 2.0, "amount": 1.5, "mode": "auto"}],
            "textSegments": [{"start": 1.0, "end": 2.0, "content": "Body label"}],
            "imageSegments": [{"start": 1.0, "end": 2.0, "path": "assets/photo.png"}],
            "keyboardSegments": [{
                "id": "keys", "start": 1.0, "end": 2.0, "displayText": "ab",
                "keys": [{"key": "a", "timeOffset": 0.0}, {"key": "b", "timeOffset": 500.0}]
            }],
            "audioSegments": [{
                "start": 0.0, "end": 10.0, "path": "assets/music.wav",
                "volumeDb": -6.0, "trimStart": 0.25, "fadeIn": 0.5, "fadeOut": 1.0
            }],
            "maskSegments": [{
                "start": 1.0, "end": 2.0, "maskType": "sensitive",
                "center": {"x": 0.5, "y": 0.5}, "size": {"x": 0.3, "y": 0.3},
                "keyframes": {"intensity": [{"time": 0.25, "value": 0.7}]}
            }],
            "camera3dSegments": [{
                "start": 1.0, "end": 2.0,
                "tracks": {"tiltX": [{"time": 0.25, "value": 10.0}]}
            }]
        },
        "captions": {
            "sourceTimed": true,
            "settings": {},
            "segments": [{
                "id": "spoken", "start": 1.0, "end": 2.0, "text": "Hello",
                "words": [{"text": "Hello", "start": 1.0, "end": 2.0}]
            }]
        },
        "markers": [{"id": "marker", "recordingSegment": 0, "time": 1.5, "label": "Body"}]
    }))
    .unwrap()
}

fn card(role: TitleCardRole, duration: f64) -> TextSegment {
    let mut segment = TextSegment::new_title_card(role, "灵录教程");
    segment.end = duration;
    segment
}

fn bounds(project: &ProjectConfiguration, role: TitleCardRole) -> (f64, f64) {
    let card = project.title_card(role).unwrap();
    (card.start, card.end)
}

#[test]
fn legacy_text_has_no_card_and_preserves_its_hold_mapping() {
    let mut project = fixture();
    let timeline = project.timeline.as_mut().unwrap();
    let segment = &mut timeline.text_segments[0];
    assert!(segment.title_card.is_none());
    assert!(value(segment).get("titleCard").is_none());
    segment.layout = TextLayout::Fullscreen;
    let before = value(&project);
    assert!(!project.normalize_title_cards().unwrap());
    assert_eq!(value(&project), before);
    let timeline = project.timeline.as_ref().unwrap();
    assert_eq!(timeline.duration(), 11.0);
    assert!(matches!(
        timeline.get_frame_mapping(1.5),
        Some(TimelineFrameMapping::Hold { .. })
    ));
    assert!(matches!(
        timeline.get_frame_mapping(3.0),
        Some(TimelineFrameMapping::Single { source, .. }) if source.source_time == 2.0
    ));
}

#[test]
fn upsert_is_role_idempotent_and_preserves_identity_and_track() {
    let mut project = fixture();
    let mut intro = card(TitleCardRole::Intro, 2.0);
    intro.track = 4;
    let first = project.upsert_title_card(intro).unwrap();
    assert!(first.changed);
    let before = value(&project);
    let mut replacement = card(TitleCardRole::Intro, 2.0);
    replacement.track = 9;
    let repeated = project.upsert_title_card(replacement.clone()).unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.id, first.id);
    assert_eq!(value(&project), before);
    replacement.content = "Updated title".into();
    assert!(project.upsert_title_card(replacement).unwrap().changed);
    assert_eq!(project.title_card(TitleCardRole::Intro).unwrap().track, 4);
    let timeline = project.timeline.as_ref().unwrap();
    assert_eq!(timeline.text_segments.len(), 2);
    assert_eq!(timeline.zoom_segments[0].start, 3.0);
}

#[test]
fn bookends_hold_recording_clock_without_shifting_music_or_source_evidence() {
    let mut project = fixture();
    synchronize_captions(&mut project, &[10.0]);
    let source = value(&project.captions);
    let markers = value(&project.markers);
    let music = value(&project.timeline.as_ref().unwrap().audio_segments);
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Intro), (0.0, 2.0));
    assert_eq!(bounds(&project, TitleCardRole::Outro), (12.0, 15.0));
    let timeline = project.timeline.as_ref().unwrap();
    assert_eq!(timeline.duration(), 15.0);
    assert_eq!(value(&timeline.audio_segments), music);
    assert_eq!(value(&project.captions), source);
    assert_eq!(value(&project.markers), markers);
    assert!(matches!(
        timeline.get_frame_mapping(0.0),
        Some(TimelineFrameMapping::Hold { source, output_end })
            if source.source_time == 0.0 && output_end == 2.0
    ));
    assert!(matches!(
        timeline.get_frame_mapping(2.5),
        Some(TimelineFrameMapping::Single { source, .. }) if source.source_time == 0.5
    ));
    assert!(matches!(
        timeline.get_frame_mapping(12.5),
        Some(TimelineFrameMapping::Hold { source, output_end })
            if source.source_time < 10.0 && source.source_time > 9.9 && output_end == 15.0
    ));
    assert_eq!(timeline.image_segments[0].start, 3.0);
    assert_eq!(timeline.mask_segments[0].start, 3.0);
    assert_eq!(timeline.mask_segments[0].keyframes.intensity[0].time, 0.25);
    assert_eq!(timeline.camera3d_segments[0].start, 3.0);
    assert_eq!(timeline.camera3d_segments[0].tracks.tilt_x[0].time, 0.25);
    assert_eq!(timeline.keyboard_segments[0].start, 3.0);
    assert_eq!(timeline.keyboard_segments[0].keys[1].time_offset, 500.0);
}

#[test]
fn source_caption_projection_does_not_apply_intro_offset_twice() {
    let mut project = fixture();
    synchronize_captions(&mut project, &[10.0]);
    let source = value(&project.captions);
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    let shifted = value(&project.timeline.as_ref().unwrap().caption_segments);
    synchronize_captions(&mut project, &[10.0]);
    assert_eq!(
        value(&project.timeline.as_ref().unwrap().caption_segments),
        shifted
    );
    project
        .set_title_card_duration(TitleCardRole::Intro, 4.0)
        .unwrap();
    synchronize_captions(&mut project, &[10.0]);
    let track = &project.timeline.as_ref().unwrap().caption_segments[0];
    assert_eq!((track.start, track.end), (5.0, 6.0));
    assert_eq!((track.words[0].start, track.words[0].end), (5.0, 6.0));
    assert_eq!(value(&project.captions), source);
    project.remove_title_card(TitleCardRole::Intro).unwrap();
    synchronize_captions(&mut project, &[10.0]);
    let track = &project.timeline.as_ref().unwrap().caption_segments[0];
    assert_eq!((track.start, track.end), (1.0, 2.0));
    assert_eq!(value(&project.captions), source);
}

#[test]
fn legacy_output_captions_migrate_back_to_the_original_source_times() {
    let mut project = fixture();
    project.captions.as_mut().unwrap().source_timed = false;
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    assert_eq!(project.captions.as_ref().unwrap().segments[0].start, 3.0);
    synchronize_captions(&mut project, &[10.0]);
    let captions = project.captions.as_ref().unwrap();
    assert!(captions.source_timed);
    assert_eq!(captions.segments[0].start, 1.0);
    assert_eq!(captions.segments[0].words[0].start, 1.0);
    assert_eq!(
        project.timeline.as_ref().unwrap().caption_segments[0].start,
        3.0
    );
    synchronize_captions(&mut project, &[10.0]);
    assert_eq!(
        project.timeline.as_ref().unwrap().caption_segments[0].start,
        3.0
    );
}

#[test]
fn outro_reanchors_after_trim_speed_append_and_delete_without_duration_drift() {
    let mut project = fixture();
    project.timeline.as_mut().unwrap().audio_segments.clear();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    let outro_id = project
        .title_card(TitleCardRole::Outro)
        .unwrap()
        .title_card
        .as_ref()
        .unwrap()
        .id
        .clone();
    let music = value(&project.timeline.as_ref().unwrap().audio_segments);
    project.timeline.as_mut().unwrap().segments[0].end = 8.0;
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    project.timeline.as_mut().unwrap().segments[0].timescale = 2.0;
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (6.0, 9.0));
    let mut clip = project.timeline.as_ref().unwrap().segments[0].clone();
    clip.end = 4.0;
    clip.timescale = 1.0;
    project.timeline.as_mut().unwrap().segments.push(clip);
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    project.timeline.as_mut().unwrap().segments.remove(0);
    assert!(project.normalize_title_cards().unwrap());
    assert_eq!(bounds(&project, TitleCardRole::Outro), (6.0, 9.0));
    assert!(!project.normalize_title_cards().unwrap());
    assert_eq!(
        value(&project.timeline.as_ref().unwrap().audio_segments),
        music
    );
    assert_eq!(
        project
            .title_card(TitleCardRole::Outro)
            .unwrap()
            .title_card
            .as_ref()
            .unwrap()
            .id,
        outro_id
    );
}

#[test]
fn outro_anchor_includes_existing_overlapping_and_closing_fullscreen_holds() {
    let mut project = fixture();
    for (start, end) in [(3.0, 5.0), (4.0, 6.0), (13.0, 14.0)] {
        project.timeline.as_mut().unwrap().text_segments.push(
            serde_json::from_value(json!({
                "start": start, "end": end, "content": "Legacy hold", "layout": "fullscreen"
            }))
            .unwrap(),
        );
    }
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (16.0, 19.0));
    assert_eq!(project.timeline.as_ref().unwrap().duration(), 19.0);
    assert!(!project.normalize_title_cards().unwrap());
}

#[test]
fn removing_bookends_restores_body_and_is_idempotent() {
    let mut project = fixture();
    let before = project.clone();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    project.remove_title_card(TitleCardRole::Intro).unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    assert!(project.remove_title_card(TitleCardRole::Outro).unwrap());
    assert!(!project.remove_title_card(TitleCardRole::Outro).unwrap());
    assert_eq!(value(&project), value(&before));
}

#[test]
fn disabled_intro_changes_hold_only_when_explicitly_enabled() {
    let mut project = fixture();
    let mut intro = card(TitleCardRole::Intro, 2.0);
    intro.enabled = false;
    project.upsert_title_card(intro).unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (10.0, 13.0));
    assert_eq!(
        project.timeline.as_ref().unwrap().zoom_segments[0].start,
        1.0
    );
    let mut intro = project.title_card(TitleCardRole::Intro).unwrap().clone();
    intro.enabled = true;
    project.upsert_title_card(intro).unwrap();
    assert_eq!(bounds(&project, TitleCardRole::Outro), (12.0, 15.0));
    assert_eq!(
        project.timeline.as_ref().unwrap().zoom_segments[0].start,
        3.0
    );
}

#[test]
fn invalid_edits_and_shortening_over_cover_overlays_leave_the_project_untouched() {
    let mut project = fixture();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    for duration in [f64::NAN, f64::INFINITY, -1.0, 0.0, 31.0] {
        let before = value(&project);
        assert_eq!(
            project.set_title_card_duration(TitleCardRole::Intro, duration),
            Err(TitleCardError::InvalidDuration)
        );
        assert_eq!(value(&project), before);
    }
    project.timeline.as_mut().unwrap().image_segments[0].start = 0.25;
    let before = value(&project);
    assert_eq!(
        project.remove_title_card(TitleCardRole::Intro),
        Err(TitleCardError::ContentBeforeIntro)
    );
    assert_eq!(value(&project), before);
    assert_eq!(
        project.set_title_card_duration(TitleCardRole::Intro, 0.5),
        Err(TitleCardError::ContentBeforeIntro)
    );
    assert_eq!(value(&project), before);
}

#[test]
fn card_metadata_rejects_unsafe_paths_unbounded_text_and_unknown_versions() {
    let segment = card(TitleCardRole::Intro, 2.0);
    for path in [
        "/tmp/photo.png",
        "../photo.png",
        "assets/../photo.png",
        "https://example.com/a.png",
        "C:\\photo.png",
        "assets//photo.png",
    ] {
        let mut data = value(&segment);
        data["titleCard"]["background"] = json!({"type": "projectImage", "path": path});
        assert!(
            serde_json::from_value::<TextSegment>(data).is_err(),
            "{path}"
        );
    }
    let mut data = value(&segment);
    data["titleCard"]["subtitle"] = json!("字".repeat(4096));
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
    let mut data = value(&segment);
    data["titleCard"]["templateVersion"] = json!(2);
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
    let mut data = value(&segment);
    data["titleCard"]["background"] =
        json!({"type": "projectImage", "path": "assets/封面.png", "focus": {"x": 1.1, "y": 0.5}});
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
}

#[test]
fn card_subject_is_optional_roundtrips_and_rejects_unsafe_paths() {
    let segment = card(TitleCardRole::Intro, 2.0);
    let legacy = value(&segment);
    assert!(legacy["titleCard"].get("subject").is_none());
    let reopened: TextSegment = serde_json::from_value(legacy).unwrap();
    assert_eq!(reopened.title_card.unwrap().subject, None);

    let mut data = value(&segment);
    data["titleCard"]["subject"] = json!({"path": "content/images/person.png"});
    let reopened: TextSegment = serde_json::from_value(data).unwrap();
    assert_eq!(
        reopened.title_card.as_ref().unwrap().subject,
        Some(TitleCardSubject {
            path: "content/images/person.png".into(),
            side: TitleCardSubjectSide::Right,
            mirror: false,
            pose: None,
        })
    );
    let encoded = value(&reopened);
    assert_eq!(encoded["titleCard"]["subject"]["side"], json!("right"));

    for path in ["/Users/me/person.png", "../person.png", "a//b.png", ""] {
        let mut data = value(&segment);
        data["titleCard"]["subject"] = json!({"path": path, "side": "left", "mirror": true});
        assert!(
            serde_json::from_value::<TextSegment>(data).is_err(),
            "{path}"
        );
    }
}

#[test]
fn project_roundtrip_preserves_card_identity_background_and_fractional_hold() {
    let mut project = fixture();
    let mut outro = card(TitleCardRole::Outro, 0.1);
    let metadata = outro.title_card.as_mut().unwrap();
    metadata.template = TitleCardTemplate::Split;
    metadata.kicker = "Quick guide".into();
    metadata.subtitle = "三步完成录制".into();
    metadata.background = TitleCardBackground::ProjectImage {
        path: "assets/封面.png".into(),
        fit: TitleCardImageFit::Cover,
        focus: XY::new(0.3, 0.6),
        dim: 0.25,
    };
    project.upsert_title_card(outro).unwrap();
    let encoded = value(&project);
    let mut reopened: ProjectConfiguration = serde_json::from_value(encoded.clone()).unwrap();
    assert!(!reopened.normalize_title_cards().unwrap());
    assert_eq!(value(&reopened), encoded);
    assert!(
        !reopened
            .set_title_card_duration(TitleCardRole::Outro, 0.1)
            .unwrap()
            .changed
    );
}

#[test]
fn duplicate_roles_and_ids_are_rejected_without_partial_repairs() {
    let mut project = fixture();
    let intro = card(TitleCardRole::Intro, 2.0);
    let mut outro = card(TitleCardRole::Outro, 3.0);
    outro.title_card.as_mut().unwrap().id = intro.title_card.as_ref().unwrap().id.clone();
    project.upsert_title_card(intro).unwrap();
    let before = value(&project);
    assert_eq!(
        project.upsert_title_card(outro),
        Err(TitleCardError::DuplicateId)
    );
    assert_eq!(value(&project), before);
    let duplicate = project.title_card(TitleCardRole::Intro).unwrap().clone();
    project
        .timeline
        .as_mut()
        .unwrap()
        .text_segments
        .push(duplicate);
    let before = value(&project);
    assert_eq!(
        project.normalize_title_cards(),
        Err(TitleCardError::DuplicateRole)
    );
    assert_eq!(value(&project), before);
}

#[test]
fn empty_footage_prevents_new_holds_but_does_not_trap_existing_cards() {
    let mut project = fixture();
    project
        .upsert_title_card(card(TitleCardRole::Intro, 2.0))
        .unwrap();
    project
        .upsert_title_card(card(TitleCardRole::Outro, 3.0))
        .unwrap();
    project.timeline.as_mut().unwrap().segments.clear();
    let before = value(&project);
    assert_eq!(
        project.normalize_title_cards(),
        Err(TitleCardError::MissingFootage)
    );
    assert_eq!(value(&project), before);
    assert!(project.remove_title_card(TitleCardRole::Intro).unwrap());
    assert!(project.remove_title_card(TitleCardRole::Outro).unwrap());
    let before = value(&project);
    assert_eq!(
        project.upsert_title_card(card(TitleCardRole::Intro, 2.0)),
        Err(TitleCardError::MissingFootage)
    );
    assert_eq!(value(&project), before);
}

#[test]
fn adding_a_card_migrates_legacy_body_text_once_without_rescaling_the_card() {
    let mut project = fixture();
    project.text_size_version = 0;
    project.text_anim_version = 0;
    let label = &mut project.timeline.as_mut().unwrap().text_segments[0];
    label.font_size = 60.0;
    label.size.y = 0.4;
    project
        .upsert_title_card(TextSegment::new_title_card(TitleCardRole::Intro, "封面"))
        .unwrap();
    assert_eq!(
        project.timeline.as_ref().unwrap().text_segments[0].font_size,
        120.0
    );
    assert_eq!(
        project.title_card(TitleCardRole::Intro).unwrap().font_size,
        96.0
    );
    assert!(!project.migrate_text_settings());
    assert_eq!(
        project.timeline.as_ref().unwrap().text_segments[0].font_size,
        120.0
    );
    let mut reopened: ProjectConfiguration = serde_json::from_value(value(&project)).unwrap();
    assert!(!reopened.migrate_text_settings());
    assert_eq!(
        reopened.title_card(TitleCardRole::Intro).unwrap().font_size,
        96.0
    );
}

#[test]
fn cover_layers_are_optional_roundtrip_and_reject_unsafe_values() {
    let segment = card(TitleCardRole::Intro, 2.0);
    let legacy = value(&segment);
    assert!(legacy["titleCard"].get("layers").is_none());
    let reopened: TextSegment = serde_json::from_value(legacy).unwrap();
    assert!(reopened.title_card.unwrap().layers.is_empty());

    let pose = json!({"center": {"x": 0.3, "y": 0.4}, "size": {"x": 0.2, "y": 0.1}});
    let mut data = value(&segment);
    data["titleCard"]["layers"] = json!([
        {"id": "a1", "pose": pose, "kind": {"type": "text", "content": "震惊"}},
        {"id": "b2", "pose": pose, "kind": {"type": "image", "path": "content/images/x.png", "mirror": true}},
    ]);
    let reopened: TextSegment = serde_json::from_value(data).unwrap();
    let layers = &reopened.title_card.as_ref().unwrap().layers;
    assert_eq!(layers.len(), 2);
    assert!(matches!(
        &layers[0].kind,
        CoverLayerKind::Text { color, stroke: None, .. } if color == "#FFFFFF"
    ));
    let encoded = value(&reopened);
    assert_eq!(
        encoded["titleCard"]["layers"][1]["kind"]["type"],
        json!("image")
    );
    assert_eq!(
        serde_json::from_value::<TextSegment>(encoded).unwrap(),
        reopened
    );

    let bad = [
        json!({"id": "a", "pose": pose, "kind": {"type": "image", "path": "../x.png"}}),
        json!({"id": "a b", "pose": pose, "kind": {"type": "text", "content": "x"}}),
        json!({"id": "a", "pose": pose, "kind": {"type": "text", "content": "x", "color": "red"}}),
        json!({"id": "a", "pose": {"center": {"x": 0.5, "y": 0.5}, "size": {"x": 0.0, "y": 0.1}},
               "kind": {"type": "text", "content": "x"}}),
    ];
    for layer in bad {
        let mut data = value(&segment);
        data["titleCard"]["layers"] = json!([layer]);
        assert!(serde_json::from_value::<TextSegment>(data).is_err());
    }
    let mut data = value(&segment);
    data["titleCard"]["layers"] = json!(
        (0..=MAX_COVER_LAYERS)
            .map(|i| json!({"id": format!("l{i}"), "pose": pose, "kind": {"type": "text", "content": "x"}}))
            .collect::<Vec<_>>()
    );
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
}

#[test]
fn cover_image_crop_and_lock_round_trip_with_custom_text_sizing() {
    let segment = card(TitleCardRole::Intro, 2.0);
    let mut data = value(&segment);
    let pose = json!({"center": {"x": 0.5, "y": 0.5}, "size": {"x": 0.4, "y": 0.2}, "rotation": 0.0, "opacity": 1.0});
    data["titleCard"]["layers"] = json!([
        {"id": "portrait", "pose": pose, "kind": {"type":"image", "path":"content/images/p.png"},
         "locked": true, "imageCrop": [0.25, 0.0625, 0.5, 0.25]},
        {"id": "words", "pose": pose, "kind": {"type":"text", "content":"Office", "color":"#FFFFFF", "stroke":"#111111", "band":"#000000"},
         "fontSize": 92.0, "strokeWidth": 3.0, "bandOpacity": 0.75}
    ]);
    let reopened: TextSegment = serde_json::from_value(data.clone()).unwrap();
    let layers = &reopened.title_card.as_ref().unwrap().layers;
    assert!(layers[0].locked);
    assert_eq!(layers[0].image_crop, Some([0.25, 0.0625, 0.5, 0.25]));
    assert_eq!(layers[1].font_size, Some(92.));
    assert_eq!(layers[1].stroke_width, Some(3.));
    assert_eq!(layers[1].band_opacity, Some(0.75));
    let encoded = value(&reopened);
    assert_eq!(
        serde_json::from_value::<TextSegment>(encoded).unwrap(),
        reopened
    );

    data["titleCard"]["layers"][0]["imageCrop"] = json!([0.8, 0.0, 0.5, 1.0]);
    assert!(serde_json::from_value::<TextSegment>(data.clone()).is_err());
    data["titleCard"]["layers"][0]["imageCrop"] = json!([0.25, 0.0625, 0.5, 0.25]);
    data["titleCard"]["layers"][1]["bandOpacity"] = json!(1.5);
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
}

#[test]
fn cropped_persona_uses_the_visible_source_aspect_after_changing_expression() {
    let mut layer = CoverLayer::new(
        LayerPose {
            center: XY::new(0.2056, 0.707),
            size: XY::new(0.2625, 0.4947),
            rotation: 0.,
            opacity: 1.,
        },
        CoverLayerKind::Image {
            path: "content/images/portrait.png".into(),
            mirror: false,
            persona: Some(CoverPersonaRef {
                persona: "hanfu".into(),
                file: "blazer-surprise.webp".into(),
            }),
        },
    );
    let crop = [0.068_777_29, 0.008_035_715, 0.885_371_2, 0.7];
    layer.image_crop = Some(crop);
    let source = (916, 1120);
    let visible_aspect = 916. * f64::from(crop[2]) / (1120. * f64::from(crop[3]));
    for output in [(1218, 790), (1920, 1080), (1080, 1920)] {
        let corrected = layer.display_pose(source, output);
        let drawn_aspect =
            corrected.size.x * f64::from(output.0) / (corrected.size.y * f64::from(output.1));
        assert!((drawn_aspect - visible_aspect).abs() < 1e-9);
        assert_eq!(corrected.center, layer.pose.center);
        assert_eq!(corrected.size.y, layer.pose.size.y);
    }
    if let CoverLayerKind::Image { persona, .. } = &mut layer.kind {
        *persona = None;
    }
    assert_eq!(layer.display_pose(source, (1218, 790)), layer.pose);
}

#[test]
fn a_card_has_at_most_one_title_layer_and_v1_layers_still_read() {
    let segment = card(TitleCardRole::Intro, 2.0);
    let pose = json!({"center": {"x": 0.5, "y": 0.5}, "size": {"x": 0.5, "y": 0.2}});
    let mut data = value(&segment);
    data["titleCard"]["layers"] = json!([
        {"id": "t", "pose": pose, "kind": {"type": "title", "stroke": "#111111", "font": "Smiley Sans"}},
        {"id": "v1", "pose": pose, "kind": {"type": "text", "content": "旧", "color": "#FFFFFF"}},
        {"id": "p", "pose": pose, "kind": {"type": "image", "path": "content/images/p.png",
            "persona": {"persona": "hanfu", "file": "hanfu-smile.webp"}}},
    ]);
    let reopened: TextSegment = serde_json::from_value(data.clone()).unwrap();
    let card = reopened.title_card.as_ref().unwrap();
    assert!(card.has_title_layer());
    assert_eq!(
        card.layers[0].kind.text_style().unwrap().font.as_deref(),
        Some("Smiley Sans")
    );
    assert!(matches!(
        &card.layers[1].kind,
        CoverLayerKind::Text { font: None, .. }
    ));

    data["titleCard"]["layers"][1] = json!({"id": "t2", "pose": pose, "kind": {"type": "title"}});
    assert!(serde_json::from_value::<TextSegment>(data.clone()).is_err());
    data["titleCard"]["layers"][1] = json!({"id": "x", "pose": pose,
        "kind": {"type": "image", "path": "content/images/p.png", "persona": {"persona": "../x", "file": "a"}}});
    assert!(serde_json::from_value::<TextSegment>(data).is_err());
}
