//! Colour conversions ported from ui/src/lib/color.ts.
use crate::{AppWindow, ColorTools, ColorValue};
use slint::{ComponentHandle, Model};

pub fn init(ui: &AppWindow) {
    let api = ui.global::<ColorTools>();
    api.on_parse(|text| parse(&text).unwrap_or_default());
    api.on_edit(|text| {
        if text.trim().trim_start_matches('#').len() == 6 {
            parse(&text).unwrap_or_default()
        } else {
            ColorValue::default()
        }
    });
    api.on_contains(|model, color| model.iter().any(|swatch| swatch.hex == color));
    api.on_hsv(hsv);
    api.on_channel(|current, channel, value| {
        let number = value.trim().parse::<f32>().ok().filter(|n| n.is_finite());
        let Some(number) = number else { return current };
        match channel.as_str() {
            "R" => rgb(number, current.green as f32, current.blue as f32),
            "G" => rgb(current.red as f32, number, current.blue as f32),
            "B" => rgb(current.red as f32, current.green as f32, number),
            "H" | "S" | "L" => {
                let (h, s, l) = match channel.as_str() {
                    "H" => (number, current.hsl_s, current.hsl_l),
                    "S" => (current.hue, number, current.hsl_l),
                    _ => (current.hue, current.hsl_s, number),
                };
                let (s, l) = (s.clamp(0.0, 100.0) / 100.0, l.clamp(0.0, 100.0) / 100.0);
                let v = l + s * l.min(1.0 - l);
                hsv(
                    h,
                    if v == 0.0 {
                        0.0
                    } else {
                        2.0 * (1.0 - l / v) * 100.0
                    },
                    v * 100.0,
                )
            }
            _ => current,
        }
    });
}
pub fn parse(text: &str) -> Option<ColorValue> {
    let text = text.trim().trim_start_matches('#');
    if ![3, 6].contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digits = if text.len() == 3 {
        text.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        text.into()
    };
    let value = u32::from_str_radix(&digits, 16).ok()?;
    Some(rgb(
        ((value >> 16) & 255) as f32,
        ((value >> 8) & 255) as f32,
        (value & 255) as f32,
    ))
}
fn rgb(r: f32, g: f32, b: f32) -> ColorValue {
    let [r, g, b] = [r, g, b].map(|n| n.clamp(0.0, 255.0).round() as u8);
    let [rr, gg, bb] = [r, g, b].map(|n| f32::from(n) / 255.0);
    let maximum = rr.max(gg).max(bb);
    let minimum = rr.min(gg).min(bb);
    let d = maximum - minimum;
    let hue = if d == 0.0 {
        0.0
    } else {
        (if maximum == rr {
            (gg - bb) / d
        } else if maximum == gg {
            (bb - rr) / d + 2.0
        } else {
            (rr - gg) / d + 4.0
        } * 60.0)
            .rem_euclid(360.0)
    };
    hsv(
        hue,
        if maximum == 0.0 {
            0.0
        } else {
            d / maximum * 100.0
        },
        maximum * 100.0,
    )
}
fn hsv(h: f32, s: f32, v: f32) -> ColorValue {
    let (h, s, v) = (
        h.rem_euclid(360.0),
        s.clamp(0.0, 100.0) / 100.0,
        v.clamp(0.0, 100.0) / 100.0,
    );
    let channel = |n: f32| {
        let k = (n + h / 60.0) % 6.0;
        (255.0 * (v - v * s * 0.0f32.max(k.min(4.0 - k).min(1.0)))).round() as u8
    };
    let (r, g, b) = (channel(5.0), channel(3.0), channel(1.0));
    let l = v * (1.0 - s / 2.0);
    let sl = if l == 0.0 || l == 1.0 {
        0.0
    } else {
        (v - l) / l.min(1.0 - l)
    };
    ColorValue {
        hex: format!("#{r:02x}{g:02x}{b:02x}").into(),
        digits: format!("{r:02x}{g:02x}{b:02x}").into(),
        color: slint::Color::from_rgb_u8(r, g, b),
        hue: h,
        saturation: s * 100.0,
        brightness: v * 100.0,
        red: i32::from(r),
        green: i32::from(g),
        blue: i32::from(b),
        hsl_s: sl * 100.0,
        hsl_l: l * 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colour_math_matches_the_original_picker() {
        let c = parse("8686fe").unwrap();
        assert_eq!((c.red, c.green, c.blue), (134, 134, 254));
        assert!((c.hue - 240.0).abs() < 0.001);
        assert_eq!(parse("#abc").unwrap().hex, "#aabbcc");
        assert!(parse("red").is_none());
        for color in [
            "#000000", "#ffffff", "#ff0000", "#00ff00", "#0000ff", "#8686fe", "#f35e62", "#2091f7",
        ] {
            let c = parse(color).unwrap();
            assert_eq!(hsv(c.hue, c.saturation, c.brightness).hex, color);
        }
        let black = hsv(240.0, 98.0, 0.0);
        assert_eq!(black.hex, "#000000");
        assert_eq!(black.hue, 240.0);
        assert_eq!(black.saturation, 98.0);
    }
}
