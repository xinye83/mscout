//! Fallback OCR with `Windows.Media.Ocr`.
//!
//! The HUD digits, name/job and map labels normally use the bitmap/font readers.
//! This engine is a last resort for renderings those readers cannot reproduce
//! (the Filter Applied name, the 1920 Default/Filtered map label). It is the
//! approach used by mpl-ocr (MIT); the implementation here is written for
//! MScout's `Gray` crops. See `THIRD_PARTY_NOTICES.md`.
//!
//! The engine is created per call: it is cheap next to the reader tick, and the
//! WinRT object is not kept on a thread that may change apartments.

use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

use crate::ocr::Gray;

/// Padding around the crop. The engine drops short second lines with too little
/// margin; 16 px was the measured minimum for the map label.
const PAD: u32 = 16;

/// Recognize the lines in a luma crop. Ink is inverted to dark text on white,
/// which is what the engine expects from a screenshot.
pub fn recognize(gray: &Gray) -> Result<Vec<String>, String> {
    if gray.w == 0 || gray.h == 0 {
        return Ok(Vec::new());
    }

    let width = gray.w as u32 + 2 * PAD;
    let height = gray.h as u32 + 2 * PAD;
    let mut pixels = vec![255u8; (width * height * 4) as usize];
    for y in 0..gray.h {
        for x in 0..gray.w {
            let value = 255u8.saturating_sub(gray.px[y * gray.w + x]);
            let index = (((y as u32 + PAD) * width + x as u32 + PAD) * 4) as usize;
            pixels[index] = value;
            pixels[index + 1] = value;
            pixels[index + 2] = value;
            pixels[index + 3] = 255;
        }
    }

    // SAFETY: idempotent per thread; an existing apartment is fine too.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let language =
        Language::CreateLanguage(&HSTRING::from("en-US")).map_err(|error| error.to_string())?;
    if !OcrEngine::IsLanguageSupported(&language).map_err(|error| error.to_string())? {
        return Err("Windows OCR has no English support".into());
    }
    let engine = OcrEngine::TryCreateFromLanguage(&language).map_err(|error| error.to_string())?;

    let buffer =
        CryptographicBuffer::CreateFromByteArray(&pixels).map_err(|error| error.to_string())?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(|error| error.to_string())?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|error| error.to_string())?;

    let mut lines = Vec::new();
    for line in result.Lines().map_err(|error| error.to_string())? {
        lines.push(line.Text().map_err(|error| error.to_string())?.to_string());
    }
    Ok(lines)
}

/// Keep the first alphanumeric run of an identity line. The name/job crop can
/// catch a neighbouring UI glyph, and the engine appends it to the line
/// (`Euroblood„JIJ`); a name or job cannot contain punctuation, so the run ends
/// there.
pub fn clean_identity(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().chars() {
        if ch.is_ascii_alphanumeric() || (ch == ' ' && !out.is_empty()) {
            out.push(ch);
        } else if !out.is_empty() {
            break;
        }
    }
    out.trim().to_string()
}

/// OCR candidates for a name or job crop. Both ink extractions are tried because
/// the key that works depends on the rendering (the Filtered job needs the white
/// key, its name needs luma). Candidates are longest-first so the caller can
/// prefer the line that explains the whole crop.
pub fn identity_candidates(
    frame: &crate::capture::Frame,
    region: crate::settings::Region,
    job: bool,
) -> Vec<String> {
    // Only the job needs a margin (the Default `Page` loses its leading `P`).
    // The name box is already around the text; expanding it picks up the panel
    // edge on the left and the job line above.
    let margin_x = if job { 5 } else { 0 };
    let margin_y = if job { 2 } else { 0 };
    let region = crate::settings::Region {
        x: region.x - margin_x,
        y: region.y - margin_y,
        w: region.w + 2 * margin_x,
        h: region.h + 2 * margin_y,
    };
    let mut out = Vec::new();
    for gray in [
        crate::ocr::crop_gray(frame, region),
        crate::ocr::crop_white_key(frame, region),
    ]
    .into_iter()
    .flatten()
    {
        let Ok(lines) = recognize(&gray) else {
            continue;
        };
        for line in lines {
            let text = clean_identity(&line);
            if text.chars().any(|ch| ch.is_ascii_alphabetic()) {
                out.push(text);
            }
        }
    }
    out.sort_by_key(|text| {
        std::cmp::Reverse(text.chars().filter(|ch| ch.is_alphanumeric()).count())
    });
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "diagnostic: Windows OCR on the folded minimap bar"]
    fn probe_folded_ocr() {
        for pair in [
            "1366x768",
            "1366x768@ideal",
            "1366x768@filtered",
            "1920x1080",
            "1920x1080@ideal",
            "1920x1080@filtered",
        ] {
            let path = crate::corpus::fixture_image(pair, "minimap-folded").0;
            let Ok(image) = image::open(&path) else {
                continue;
            };
            let image = image.to_rgba8();
            let frame = crate::capture::Frame {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            };
            let Some(minimap) = crate::ocr::detect_minimap(&frame) else {
                println!("{pair}: no minimap");
                continue;
            };
            let Some(label) = crate::ocr::folded_label(&frame, &minimap) else {
                println!("{pair}: no folded label");
                continue;
            };
            println!(
                "{pair}: {:?} {}x{} ocr={:?}",
                minimap.state,
                label.w,
                label.h,
                recognize(&label)
            );
        }
    }

    #[test]
    #[ignore = "diagnostic: Windows OCR on the Default/Filtered map label lines"]
    fn probe_map_label_ocr() {
        for pair in ["1920x1080", "1920x1080@filtered"] {
            let path = crate::corpus::fixture_image(pair, "minimap-expanded").0;
            let image = image::open(&path).expect("minimap fixture").to_rgba8();
            let frame = crate::capture::Frame {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            };
            let Some(regions) = crate::ocr::map_label_line_regions(&frame) else {
                println!("{pair}: no lines");
                continue;
            };
            println!("{pair}: {} lines", regions.len());
            for (index, (region, gray)) in regions.iter().enumerate() {
                let luma = recognize(gray);
                let white = crate::ocr::crop_white_key(&frame, *region).map(|key| recognize(&key));
                println!(
                    "  line {index} {}x{}: luma={luma:?} white={white:?}",
                    gray.w, gray.h
                );
            }
            let x0 = regions.iter().map(|(r, _)| r.x).min().unwrap_or(0) - 4;
            let y0 = regions.iter().map(|(r, _)| r.y).min().unwrap_or(0) - 4;
            let x1 = regions.iter().map(|(r, _)| r.x + r.w).max().unwrap_or(0) + 4;
            let y1 = regions.iter().map(|(r, _)| r.y + r.h).max().unwrap_or(0) + 4;
            let union = crate::settings::Region {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            };
            let white = crate::ocr::crop_white_key(&frame, union).map(|key| recognize(&key));
            println!("  union {}x{}: white={white:?}", union.w, union.h);
        }
    }

    #[test]
    #[ignore = "diagnostic: grid-search the Default job OCR crop"]
    fn probe_default_job_crops() {
        let path = crate::corpus::fixture_image("1920x1080", "hud-main").0;
        let hud = image::open(&path).expect("hud fixture").to_rgba8();
        let frame = crate::capture::Frame {
            width: hud.width(),
            height: hud.height(),
            rgba: hud.into_raw(),
        };
        for x in 104..=116 {
            for w in [60, 70, 80, 90] {
                for h in [20, 22, 24, 26] {
                    let region = crate::settings::Region { x, y: 11, w, h };
                    for (key, gray) in [
                        ("luma", crate::ocr::crop_gray(&frame, region)),
                        ("white", crate::ocr::crop_white_key(&frame, region)),
                    ] {
                        let Some(gray) = gray else { continue };
                        let Ok(lines) = recognize(&gray) else {
                            continue;
                        };
                        let cleaned: Vec<String> =
                            lines.iter().map(|line| clean_identity(line)).collect();
                        if cleaned.iter().any(|line| !line.is_empty()) {
                            println!("x={x} w={w} h={h} {key}: {cleaned:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "diagnostic: Windows OCR on the Filtered name/job fixture"]
    fn reads_the_filtered_identity_fixture() {
        let path = crate::corpus::fixture_image("1920x1080@filtered", "hud-main").0;
        let hud = image::open(&path).expect("hud fixture").to_rgba8();
        let frame = crate::capture::Frame {
            width: hud.width(),
            height: hud.height(),
            rgba: hud.into_raw(),
        };
        for (label, region, expected) in [
            (
                "job",
                crate::settings::Region {
                    x: 112,
                    y: 13,
                    w: 70,
                    h: 22,
                },
                "page",
            ),
            (
                "name",
                crate::settings::Region {
                    x: 112,
                    y: 33,
                    w: 110,
                    h: 20,
                },
                "Euroblood",
            ),
        ] {
            let mut found = false;
            for (key, gray) in [
                ("luma", crate::ocr::crop_gray(&frame, region)),
                ("white", crate::ocr::crop_white_key(&frame, region)),
            ] {
                let Some(gray) = gray else { continue };
                let lines = recognize(&gray).expect("ocr");
                println!("{label} {key}: {lines:?}");
                found |= lines
                    .iter()
                    .map(|line| clean_identity(line))
                    .any(|line| line.eq_ignore_ascii_case(expected));
            }
            assert!(found, "{label}: expected {expected:?} from one key");
        }
    }
}
