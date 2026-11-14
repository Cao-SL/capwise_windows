//! Personas: sets of cutout portraits a cover can stand a person in.
//!
//! A persona is a folder holding `persona.json` and the cutouts it lists. The
//! app ships public ones under `assets/personas`; the user's own live in the
//! app data folder and never enter a project file by their real path. Placing
//! a pose copies it into the project's `content/images`, so a project opens
//! and exports the same on any Mac.

use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use cap_project::TitleCardSubjectSide;
use image::{GenericImageView as _, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::asset_library::{self, AssetKind, AssetRecord};

const MANIFEST: &str = "persona.json";
const MANIFEST_VERSION: u32 = 1;
pub(crate) const PRIVATE_ID: &str = "mine";
const MAX_POSE_HEIGHT: u32 = 1200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Expression {
    Smile,
    Surprise,
    Think,
    Point,
    Confused,
    ThumbsUp,
    Photo,
}

/// Which side of the frame the person turns, looks or points toward. A cover
/// turns them toward the title, mirroring the cutout when needed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Facing {
    #[default]
    Left,
    Right,
    Front,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pose {
    pub file: String,
    pub expression: Expression,
    #[serde(default)]
    pub outfit: String,
    #[serde(default)]
    pub facing: Facing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_en: Option<String>,
    #[serde(default)]
    pub poses: Vec<Pose>,
    /// Original photos kept for generating new poses later.
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Persona {
    pub id: String,
    pub dir: PathBuf,
    pub builtin: bool,
    pub manifest: Manifest,
}

impl Persona {
    pub(crate) fn display_name(&self) -> &str {
        match (crate::i18n::current(), &self.manifest.name_en) {
            (crate::i18n::Locale::ZhCn, _) | (_, None) => &self.manifest.name,
            (_, Some(name)) => name,
        }
    }

    pub(crate) fn pose_path(&self, pose: &Pose) -> Option<PathBuf> {
        safe_file_name(&pose.file).then(|| self.dir.join(&pose.file))
    }
}

fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

pub(crate) fn bundled_dir() -> Option<PathBuf> {
    let override_dir = std::env::var_os("CAP_GPUI_PERSONAS_DIR").map(PathBuf::from);
    if let Some(path) = override_dir.filter(|path| path.is_dir()) {
        return Some(path);
    }
    crate::store::bundled_resource_dirs()
        .into_iter()
        .map(|directory| directory.join("assets/personas"))
        .chain([Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/personas")])
        .find(|path| path.is_dir())
}

pub(crate) fn private_dir() -> PathBuf {
    crate::store::app_data_dir().join("personas")
}

/// Every persona with at least one usable pose: the bundled ones first, then
/// the user's own.
pub(crate) fn all() -> Vec<Persona> {
    let mut personas = bundled_dir()
        .map(|root| load_from(&root, true))
        .unwrap_or_default();
    personas.extend(load_from(&private_dir(), false));
    personas
}

fn load_from(root: &Path, builtin: bool) -> Vec<Persona> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut directories = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    directories.sort();
    directories
        .into_iter()
        .filter_map(|dir| {
            let id = dir.file_name()?.to_str()?.to_owned();
            let manifest = read_manifest(&dir)?;
            let mut persona = Persona {
                id,
                dir,
                builtin,
                manifest,
            };
            let dir = persona.dir.clone();
            persona
                .manifest
                .poses
                .retain(|pose| safe_file_name(&pose.file) && dir.join(&pose.file).is_file());
            (!persona.manifest.poses.is_empty()).then_some(persona)
        })
        .collect()
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    let bytes = fs::read(dir.join(MANIFEST)).ok()?;
    let manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    (manifest.version == MANIFEST_VERSION).then_some(manifest)
}

/// Whether the cutout has to be mirrored so a person standing on `side`
/// turns toward the title on the other side.
pub(crate) fn mirror_for(facing: Facing, side: TitleCardSubjectSide) -> bool {
    matches!(
        (facing, side),
        (Facing::Left, TitleCardSubjectSide::Left) | (Facing::Right, TitleCardSubjectSide::Right)
    )
}

/// The side a pose stands on by default: away from where it points.
pub(crate) fn default_side(facing: Facing) -> TitleCardSubjectSide {
    match facing {
        Facing::Right => TitleCardSubjectSide::Left,
        Facing::Left | Facing::Front => TitleCardSubjectSide::Right,
    }
}

/// Copies a pose into the project as an outlined PNG and records it as a
/// project asset. Blocking: run it on a background executor.
pub(crate) fn place_in_project(
    project: &Path,
    persona: &Persona,
    pose: &Pose,
) -> Result<AssetRecord, String> {
    let source = persona
        .pose_path(pose)
        .ok_or_else(|| "This pose is missing".to_string())?;
    let image = image::open(&source)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let radius = (image.height() / 110).clamp(3, 14);
    let outlined = outlined(&image, radius);
    let mut bytes = Vec::new();
    outlined
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|error| error.to_string())?;
    let name = persona.display_name().to_owned();
    let image = cap_project::OverlayImage::from_bytes(bytes, name)?;
    let imported = crate::import::save_editor_image(project, image)?;
    asset_library::ensure_record(
        project,
        &AssetRecord {
            path: imported.path,
            name: imported.name,
            kind: AssetKind::Photo,
            source: None,
            rendition: None,
        },
    )
}

/// A white rim around the cutout's silhouette: the sticker edge that keeps
/// a person readable on a busy or same-colored background.
pub(crate) fn outlined(image: &RgbaImage, radius: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    let pad = radius as usize;
    let out_width = width as usize + pad * 2;
    let out_height = height as usize + pad * 2;
    let mut alpha = vec![0u8; out_width * out_height];
    for (x, y, pixel) in image.enumerate_pixels() {
        alpha[(y as usize + pad) * out_width + x as usize + pad] = pixel[3];
    }
    let dilated = dilate(&alpha, out_width, out_height, pad);
    let mut output = RgbaImage::new(out_width as u32, out_height as u32);
    for (index, pixel) in output.pixels_mut().enumerate() {
        *pixel = image::Rgba([255, 255, 255, dilated[index]]);
    }
    for (x, y, source) in image.enumerate_pixels() {
        let target = output.get_pixel_mut(x + radius, y + radius);
        let alpha = u32::from(source[3]);
        let inverse = 255 - alpha;
        for channel in 0..3 {
            target[channel] = ((u32::from(source[channel]) * alpha
                + u32::from(target[channel]) * inverse)
                / 255) as u8;
        }
        target[3] = (alpha + u32::from(target[3]) * inverse / 255).min(255) as u8;
    }
    output
}

/// A round dilation: every row's sliding maximum over each chord width of
/// a disc, combined across the disc's rows.
fn dilate(alpha: &[u8], width: usize, height: usize, radius: usize) -> Vec<u8> {
    if radius == 0 || width == 0 {
        return alpha.to_vec();
    }
    let chords = (0..=radius)
        .map(|dy| (((radius * radius - dy * dy) as f64).sqrt()) as usize)
        .collect::<Vec<_>>();
    let mut reaches = chords.clone();
    reaches.sort_unstable();
    reaches.dedup();
    let horizontal = reaches
        .iter()
        .map(|&reach| {
            let mut rows = vec![0u8; alpha.len()];
            for (source, target) in alpha.chunks(width).zip(rows.chunks_mut(width)) {
                sliding_max(source, reach, target);
            }
            (reach, rows)
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut output = vec![0u8; alpha.len()];
    for y in 0..height {
        let target = &mut output[y * width..(y + 1) * width];
        for (dy, chord) in chords.iter().enumerate() {
            let rows = &horizontal[chord];
            for source_y in [y.checked_sub(dy), (y + dy < height).then_some(y + dy)]
                .into_iter()
                .flatten()
            {
                let source = &rows[source_y * width..(source_y + 1) * width];
                for (value, candidate) in target.iter_mut().zip(source) {
                    *value = (*value).max(*candidate);
                }
            }
        }
    }
    output
}

/// The maximum of each centered window of `2 * reach + 1` samples.
fn sliding_max(row: &[u8], reach: usize, output: &mut [u8]) {
    let mut window = std::collections::VecDeque::<usize>::new();
    for index in 0..row.len() + reach {
        if index < row.len() {
            while window.back().is_some_and(|&last| row[last] <= row[index]) {
                window.pop_back();
            }
            window.push_back(index);
        }
        let Some(center) = index.checked_sub(reach) else {
            continue;
        };
        while window.front().is_some_and(|&first| first + reach < center) {
            window.pop_front();
        }
        output[center] = window.front().map_or(0, |&first| row[first]);
    }
}

/// Adds photos of the user to their private persona: each original is kept
/// as a reference for generating poses later, and a cutout of it becomes a
/// pose right away where this Mac can cut a person out. Blocking.
pub(crate) fn import_photos(sources: &[PathBuf], name: &str) -> Result<ImportSummary, String> {
    let dir = private_dir().join(PRIVATE_ID);
    fs::create_dir_all(dir.join("references")).map_err(|error| error.to_string())?;
    let mut manifest = read_manifest(&dir).unwrap_or_else(|| Manifest {
        version: MANIFEST_VERSION,
        name: name.to_owned(),
        name_en: None,
        poses: Vec::new(),
        references: Vec::new(),
    });
    let mut summary = ImportSummary::default();
    for source in sources {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|extension| {
                matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "heic")
            })
            .ok_or_else(|| "Choose a PNG, JPEG, WebP or HEIC photo".to_string())?;
        let reference = format!("references/{id}.{extension}");
        fs::copy(source, dir.join(&reference)).map_err(|error| error.to_string())?;
        manifest.references.push(reference);
        summary.references += 1;
        match crate::persona_cutout::cut_out(source) {
            Ok(cutout) => {
                let file = format!("photo-{id}.png");
                fit_height(cutout, MAX_POSE_HEIGHT)
                    .save(dir.join(&file))
                    .map_err(|error| error.to_string())?;
                manifest.poses.push(Pose {
                    file,
                    expression: Expression::Photo,
                    outfit: String::new(),
                    facing: Facing::Front,
                });
                summary.poses += 1;
            }
            Err(error) => {
                tracing::warn!(%error, "could not cut the person out of an imported photo");
                summary.uncut += 1;
            }
        }
    }
    write_manifest(&dir, &manifest)?;
    Ok(summary)
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportSummary {
    pub references: usize,
    pub poses: usize,
    pub uncut: usize,
}

fn fit_height(image: RgbaImage, maximum: u32) -> RgbaImage {
    let image = trim_transparent(image);
    if image.height() <= maximum {
        return image;
    }
    let width = (u64::from(image.width()) * u64::from(maximum) / u64::from(image.height())) as u32;
    image::imageops::resize(
        &image,
        width.max(1),
        maximum,
        image::imageops::FilterType::Lanczos3,
    )
}

fn trim_transparent(image: RgbaImage) -> RgbaImage {
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] > 8 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
    }
    if left > right || top > bottom {
        return image;
    }
    image
        .view(left, top, right - left + 1, bottom - top + 1)
        .to_image()
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
    let mut staged = tempfile::NamedTempFile::new_in(dir).map_err(|error| error.to_string())?;
    staged
        .write_all(&bytes)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    staged
        .persist(dir.join(MANIFEST))
        .map(|_| ())
        .map_err(|error| error.error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(file: &str, facing: Facing) -> Pose {
        Pose {
            file: file.into(),
            expression: Expression::Smile,
            outfit: "hanfu".into(),
            facing,
        }
    }

    #[test]
    fn manifest_parses_and_keeps_only_existing_safe_poses() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hanfu");
        fs::create_dir_all(&dir).unwrap();
        RgbaImage::new(2, 2).save(dir.join("smile.png")).unwrap();
        fs::write(
            dir.join(MANIFEST),
            serde_json::to_vec(&Manifest {
                version: MANIFEST_VERSION,
                name: "汉服女生".into(),
                name_en: Some("Hanfu".into()),
                poses: vec![
                    pose("smile.png", Facing::Left),
                    pose("missing.png", Facing::Left),
                    pose("../escape.png", Facing::Left),
                ],
                references: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();
        fs::create_dir_all(root.path().join("empty")).unwrap();
        let personas = load_from(root.path(), true);
        assert_eq!(personas.len(), 1);
        assert_eq!(personas[0].id, "hanfu");
        assert_eq!(
            personas[0].manifest.poses,
            vec![pose("smile.png", Facing::Left)]
        );
    }

    #[test]
    fn people_turn_toward_the_title() {
        assert!(!mirror_for(Facing::Left, TitleCardSubjectSide::Right));
        assert!(mirror_for(Facing::Left, TitleCardSubjectSide::Left));
        assert!(mirror_for(Facing::Right, TitleCardSubjectSide::Right));
        assert!(!mirror_for(Facing::Front, TitleCardSubjectSide::Left));
        assert_eq!(default_side(Facing::Left), TitleCardSubjectSide::Right);
        assert_eq!(default_side(Facing::Right), TitleCardSubjectSide::Left);
    }

    #[test]
    fn outline_rims_the_silhouette_without_covering_it() {
        let mut image = RgbaImage::new(5, 5);
        image.put_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let outlined = outlined(&image, 2);
        assert_eq!(outlined.dimensions(), (9, 9));
        assert_eq!(outlined.get_pixel(4, 4), &image::Rgba([10, 20, 30, 255]));
        assert_eq!(outlined.get_pixel(4, 2), &image::Rgba([255, 255, 255, 255]));
        assert_eq!(outlined.get_pixel(0, 0)[3], 0);
    }

    #[test]
    fn sliding_max_matches_a_direct_window() {
        let row = [3u8, 0, 9, 1, 4, 4, 0, 7];
        for reach in 0..4 {
            let mut fast = [0u8; 8];
            sliding_max(&row, reach, &mut fast);
            let direct = (0..row.len())
                .map(|x| {
                    *row[x.saturating_sub(reach)..(x + reach + 1).min(row.len())]
                        .iter()
                        .max()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            assert_eq!(fast.to_vec(), direct, "reach {reach}");
        }
    }

    #[test]
    fn trim_drops_transparent_margins() {
        let mut image = RgbaImage::new(10, 10);
        image.put_pixel(3, 4, image::Rgba([0, 0, 0, 255]));
        image.put_pixel(6, 8, image::Rgba([0, 0, 0, 255]));
        assert_eq!(trim_transparent(image).dimensions(), (4, 5));
    }
}
