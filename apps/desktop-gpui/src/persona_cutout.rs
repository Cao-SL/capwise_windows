//! Cuts the person out of a still photo with Apple Vision's foreground
//! instance mask (macOS 14+), at the photo's full resolution.

use std::path::Path;

use image::RgbaImage;

#[cfg(target_os = "macos")]
pub(crate) fn cut_out(path: &Path) -> Result<RgbaImage, String> {
    use cidre::{arc, ns, objc, vn};

    // Looked up at runtime: the class is missing before macOS 14, where the
    // photo is still kept as a reference, just not cut out.
    let class =
        unsafe { objc::objc_getClass(c"VNGenerateForegroundInstanceMaskRequest".as_ptr().cast()) }
            .ok_or_else(|| "Cutting out a person needs macOS 14 or later".to_string())?;
    let class: &objc::Class<vn::GenForegroundInstanceMaskRequest> =
        unsafe { std::mem::transmute(class) };
    let path = path
        .to_str()
        .ok_or_else(|| "The photo path is not valid UTF-8".to_string())?;
    objc::ar_pool(|| {
        let request: arc::R<vn::GenForegroundInstanceMaskRequest> = unsafe { class.new() };
        let url = ns::Url::with_fs_path_str(path, false);
        let handler = vn::ImageRequestHandler::with_url(&url, None);
        let requests = ns::Array::<vn::Request>::from_slice(&[request.as_ref()]);
        handler
            .perform(&requests)
            .map_err(|error| format!("Vision could not read the photo: {error:?}"))?;
        let results = request
            .results()
            .ok_or_else(|| "No person was found in the photo".to_string())?;
        let observation = results
            .first()
            .ok_or_else(|| "No person was found in the photo".to_string())?;
        let mut masked = observation
            .generate_masked_image_for_instances_cropped(
                &observation.all_instances(),
                &handler,
                true,
            )
            .map_err(|error| format!("Vision could not cut the person out: {error:?}"))?;
        read_pixels(&mut masked)
    })
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn CVPixelBufferGetBaseAddress(pixel_buffer: &cidre::cv::PixelBuf) -> *mut std::ffi::c_void;
    fn CVPixelBufferGetBytesPerRow(pixel_buffer: &cidre::cv::PixelBuf) -> usize;
}

#[cfg(target_os = "macos")]
fn read_pixels(buffer: &mut cidre::cv::PixelBuf) -> Result<RgbaImage, String> {
    use cidre::cv;

    let width = buffer.width();
    let height = buffer.height();
    let format = buffer.pixel_format();
    let flags = cv::pixel_buffer::LockFlags::READ_ONLY;
    unsafe { buffer.lock_base_addr(flags) }
        .result()
        .map_err(|error| format!("lock pixel buffer: {error:?}"))?;
    let base = unsafe { CVPixelBufferGetBaseAddress(buffer) }.cast::<u8>();
    let stride = unsafe { CVPixelBufferGetBytesPerRow(buffer) };
    let result = if base.is_null() {
        Err("The cutout has no pixels".to_string())
    } else {
        decode_rows(base, stride, width, height, format)
    };
    unsafe { buffer.unlock_lock_base_addr(flags) };
    result
}

#[cfg(target_os = "macos")]
fn decode_rows(
    base: *const u8,
    stride: usize,
    width: usize,
    height: usize,
    format: cidre::cv::PixelFormat,
) -> Result<RgbaImage, String> {
    use cidre::cv;

    let bytes_per_pixel = match format {
        cv::PixelFormat::_32_BGRA | cv::PixelFormat::_32_RGBA => 4,
        cv::PixelFormat::_64_RGBA_HALF => 8,
        _ => {
            return Err(format!(
                "Unexpected cutout pixel format {}",
                String::from_utf8_lossy(&format.0.to_be_bytes())
            ));
        }
    };
    let mut image = RgbaImage::new(width as u32, height as u32);
    for y in 0..height {
        let row =
            unsafe { std::slice::from_raw_parts(base.add(y * stride), width * bytes_per_pixel) };
        for (x, p) in row.chunks_exact(bytes_per_pixel).enumerate() {
            let rgba = match format {
                cv::PixelFormat::_32_BGRA => [p[2], p[1], p[0], p[3]],
                cv::PixelFormat::_32_RGBA => [p[0], p[1], p[2], p[3]],
                _ => {
                    let channel = |index: usize| {
                        let bits = u16::from_ne_bytes([p[index * 2], p[index * 2 + 1]]);
                        (half_to_f32(bits).clamp(0.0, 1.0) * 255.0).round() as u8
                    };
                    [channel(0), channel(1), channel(2), channel(3)]
                }
            };
            image.put_pixel(x as u32, y as u32, image::Rgba(unpremultiply(rgba)));
        }
    }
    Ok(image)
}

/// Vision's masked image is premultiplied: transparent edges darken unless
/// the color is divided back out.
fn unpremultiply([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    if a == 0 {
        return [0, 0, 0, 0];
    }
    let scale =
        |channel: u8| ((u32::from(channel) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
    [scale(r), scale(g), scale(b), a]
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x03ff);
    match exponent {
        0 => sign * fraction * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn cut_out(_path: &Path) -> Result<RgbaImage, String> {
    Err("Cutting out a person is only available on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplied_edges_recover_their_color() {
        assert_eq!(unpremultiply([64, 32, 0, 128]), [128, 64, 0, 128]);
        assert_eq!(unpremultiply([9, 9, 9, 0]), [0, 0, 0, 0]);
        assert_eq!(unpremultiply([200, 100, 50, 255]), [200, 100, 50, 255]);
    }

    #[test]
    fn half_floats_decode() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0), 0.0);
    }
}
