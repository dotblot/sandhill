//! Color arithmetic for token derivation.
//!
//! Mixing happens in OKLab so a step of the same size reads the same on a
//! light and a dark surface. Hex parsing follows GPUI's `#rgb`, `#rgba`,
//! `#rrggbb`, and `#rrggbbaa` forms.

use gpui_kit::{Hsla, Rgba};

/// Parses a CSS hex color. Panics on an invalid literal, so use it only for
/// constants. User input goes through [`try_hex`].
pub fn hex(value: &str) -> Hsla {
    try_hex(value).unwrap_or_else(|error| panic!("{error}"))
}

/// Parses a CSS hex color.
pub fn try_hex(value: &str) -> Result<Hsla, String> {
    Rgba::try_from(value)
        .map(Hsla::from)
        .map_err(|error| error.to_string())
}

/// Formats a color as `#rrggbb`, or `#rrggbbaa` when it is not opaque.
pub fn to_hex(color: Hsla) -> String {
    let rgba = color.to_rgb();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    let (r, g, b, a) = (
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b),
        channel(rgba.a),
    );
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// Mixes `from` toward `to` by `amount` in OKLab. `0.` is `from`, `1.` is `to`.
/// Alpha interpolates linearly.
pub fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let amount = amount.clamp(0., 1.);
    let a = to_oklab(from);
    let b = to_oklab(to);
    let lab = [
        a[0] + (b[0] - a[0]) * amount,
        a[1] + (b[1] - a[1]) * amount,
        a[2] + (b[2] - a[2]) * amount,
    ];
    let alpha = from.a + (to.a - from.a) * amount;
    from_oklab(lab, alpha)
}

/// OKLab lightness in `0..=1`.
pub fn lightness(color: Hsla) -> f32 {
    to_oklab(color)[0]
}

fn to_oklab(color: Hsla) -> [f32; 3] {
    let rgba = color.to_rgb();
    let r = srgb_to_linear(rgba.r);
    let g = srgb_to_linear(rgba.g);
    let b = srgb_to_linear(rgba.b);

    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();

    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab(lab: [f32; 3], alpha: f32) -> Hsla {
    let [big_l, a, b] = lab;
    let l = big_l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = big_l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = big_l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l = l * l * l;
    let m = m * m * m;
    let s = s * s * s;

    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;

    Hsla::from(Rgba {
        r: linear_to_srgb(r),
        g: linear_to_srgb(g),
        b: linear_to_srgb(b),
        a: alpha,
    })
}

/// The WCAG 2 contrast ratio between two opaque colors, from 1 to 21.
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let luminance = |color: Hsla| {
        let rgb = color.to_rgb();
        0.2126 * srgb_to_linear(rgb.r)
            + 0.7152 * srgb_to_linear(rgb.g)
            + 0.0722 * srgb_to_linear(rgb.b)
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    let value = value.clamp(0., 1.);
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1. / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for literal in ["#ffffff", "#181818", "#339cff", "#1a1c1f", "#00a240"] {
            assert_eq!(to_hex(hex(literal)), literal);
        }
        assert_eq!(to_hex(hex("#339cff80")), "#339cff80");
        assert!(try_hex("339cff").is_err());
    }

    #[test]
    fn mix_endpoints_and_midpoint() {
        let white = hex("#ffffff");
        let black = hex("#000000");
        assert_eq!(to_hex(mix(white, black, 0.)), "#ffffff");
        assert_eq!(to_hex(mix(white, black, 1.)), "#000000");
        // OKLab lightness is perceptual: the midpoint is a mid gray, not sRGB 50%.
        let mid = lightness(mix(white, black, 0.5));
        assert!((mid - 0.5).abs() < 0.02, "midpoint lightness {mid}");
    }

    #[test]
    fn a_small_step_toward_ink_stays_near_the_surface() {
        let surface = hex("#ffffff");
        let ink = hex("#1a1c1f");
        let step = mix(surface, ink, 0.05);
        let l = lightness(step);
        assert!(l > 0.94 && l < 0.98, "lightness {l}");
        let surface = hex("#181818");
        let ink = hex("#ffffff");
        let step = mix(surface, ink, 0.05);
        assert!(lightness(step) > lightness(surface));
    }
}
