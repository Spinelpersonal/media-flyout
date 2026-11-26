use super::Palette;
use crate::config::{ControlColorMode, DEFAULT_CONTROL_COLOR, Settings};
use slint::Color;

fn luminance(rgb: [u8; 3]) -> f32 {
    let [r, g, b] = rgb.map(|channel| {
        let s = f32::from(channel) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

// Preserve the original color whenever it already reads against the surface.
// Otherwise tint or shade it in small steps until it meets the text contrast target.
fn readable(accent: [u8; 3], background: f32, light: bool) -> [u8; 3] {
    let end = if light { 0 } else { 255 };
    for step in 0..32 {
        let rgb = accent.map(|v| ((i32::from(v) * (32 - step) + end * step) / 32) as u8);
        if contrast(luminance(rgb), background) >= 4.5 {
            return rgb;
        }
    }
    [end as u8; 3]
}

pub(super) fn apply(palette: &Palette, settings: &Settings, cover: Option<[u8; 3]>) {
    let accent = match settings.control_color_mode {
        ControlColorMode::Default => DEFAULT_CONTROL_COLOR,
        ControlColorMode::Artwork => cover.unwrap_or(DEFAULT_CONTROL_COLOR),
        ControlColorMode::Custom => settings.custom_control_color,
    };
    let [r, g, b] = accent;
    palette.set_accent(Color::from_rgb_u8(r, g, b));
    let l = luminance(accent);
    let ink = if contrast(l, 0.0) >= contrast(l, 1.0) {
        0
    } else {
        255
    };
    palette.set_on_accent(Color::from_rgb_u8(ink, ink, ink));
    let surface = palette.get_pressed();
    let [r, g, b] = readable(
        accent,
        luminance([surface.red(), surface.green(), surface.blue()]),
        palette.get_light(),
    );
    palette.set_accent_text(Color::from_rgb_u8(r, g, b));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adjusts_extreme_covers_for_both_themes() {
        for (light, bg) in [(true, [198, 210, 220]), (false, [60, 62, 74])] {
            for rgb in [[0; 3], [255; 3], [5, 10, 160], [250, 240, 80]] {
                let adjusted = readable(rgb, luminance(bg), light);
                assert!(contrast(luminance(adjusted), luminance(bg)) >= 4.5);
                if contrast(luminance(rgb), luminance(bg)) >= 4.5 {
                    assert_eq!(adjusted, rgb);
                }
            }
        }
    }

    #[test]
    fn rendered_controls_apply_each_colour_mode_in_both_themes() {
        use crate::ui::Flyout;
        use slint::{
            ComponentHandle, PhysicalSize, PlatformError,
            platform::{
                Platform, WindowAdapter,
                software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
            },
        };
        use std::rc::Rc;

        struct Headless;
        impl Platform for Headless {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
                Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
            }
        }
        slint::platform::set_platform(Box::new(Headless)).unwrap();
        let flyout = Flyout::new().unwrap();
        flyout.window().set_size(PhysicalSize::new(452, 196));
        flyout.set_can_toggle(true);
        flyout.set_has_timeline(true);
        flyout.set_progress(0.75);
        flyout.set_spectrum(
            Rc::new(slint::VecModel::from(vec![
                12;
                crate::media::visualizer::BARS
            ]))
            .into(),
        );
        flyout.show().unwrap();
        let palette = flyout.global::<Palette>();
        flyout.set_volume_available(true);
        flyout.set_app_volume(64);
        for light in [false, true] {
            palette.set_light(light);
            let cover = [15, 60, 150];
            let custom = [220, 45, 30];
            for (mode, art, expected) in [
                (ControlColorMode::Artwork, Some(cover), cover),
                (ControlColorMode::Artwork, None, DEFAULT_CONTROL_COLOR),
                (
                    ControlColorMode::Default,
                    Some(cover),
                    DEFAULT_CONTROL_COLOR,
                ),
                (ControlColorMode::Custom, Some(cover), custom),
                (ControlColorMode::Custom, None, custom),
            ] {
                let settings = Settings {
                    control_color_mode: mode,
                    custom_control_color: custom,
                    ..Settings::default()
                };
                apply(&palette, &settings, art);
                let pixels = flyout.window().take_snapshot().unwrap();
                let rgb = |x: usize, y: usize| {
                    let p = pixels.as_slice()[y * pixels.width() as usize + x];
                    [p.r, p.g, p.b]
                };
                assert_eq!(rgb(216, 137), expected);
                let bar = palette.get_accent_text();
                assert_eq!(rgb(248, 121), [bar.red(), bar.green(), bar.blue()]);
                let empty = palette.get_border();
                assert_eq!(rgb(350, 121), [empty.red(), empty.green(), empty.blue()]);
                assert_eq!(rgb(320, 167), [bar.red(), bar.green(), bar.blue()]);
                for i in 0..crate::media::visualizer::BARS {
                    assert_eq!(rgb(26 + i * 4, 173), [bar.red(), bar.green(), bar.blue()]);
                }
            }
        }
    }
}
