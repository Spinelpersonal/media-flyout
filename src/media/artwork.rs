use std::sync::Arc;

use windows::{
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat,
        BitmapTransform, ColorManagementMode, ExifOrientationMode,
    },
    Storage::Streams::IRandomAccessStreamReference,
    core::Result,
};

use super::{Artwork, worker::wait};

/// Decode through Windows Imaging Component and scale before transferring pixels.
/// Only the selected session is decoded; the retained image is at most 256 KiB.
pub(super) fn load(reference: IRandomAccessStreamReference) -> Result<Option<Arc<Artwork>>> {
    let stream = wait(reference.OpenReadAsync()?)?;
    if stream.Size()? > 8 * 1024 * 1024 {
        return Ok(None);
    }
    let decoder = wait(BitmapDecoder::CreateAsync(&stream)?)?;
    let (width, height) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 40_000_000
    {
        return Ok(None);
    }
    let ratio = (256.0 / f64::from(width.max(height))).min(1.0);
    let width = (f64::from(width) * ratio).round().max(1.0) as u32;
    let height = (f64::from(height) * ratio).round().max(1.0) as u32;
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(width)?;
    transform.SetScaledHeight(height)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    let data = wait(decoder.GetPixelDataTransformedAsync(
        BitmapPixelFormat::Rgba8,
        BitmapAlphaMode::Straight,
        &transform,
        ExifOrientationMode::IgnoreExifOrientation,
        ColorManagementMode::DoNotColorManage,
    )?)?;
    let rgba = data.DetachPixelData()?.to_vec();
    if rgba.len() != width as usize * height as usize * 4 {
        return Ok(None);
    }
    Ok(Some(Arc::new(Artwork {
        width,
        height,
        rgba,
    })))
}
