//! Recap PNGs leave the app only through a user-selected file/folder or clipboard.
use base64::Engine;
use serde::Deserialize;
use std::{borrow::Cow, io::Cursor, path::Path};
#[derive(Deserialize)]
pub struct Card {
    name: String,
    png: String,
}
const MAX_PNG: usize = 12 * 1024 * 1024;
fn decode(card: &Card) -> Result<(Vec<u8>, Vec<u8>), String> {
    if !card.name.starts_with("pulse-recap-")
        || !card.name.ends_with(".png")
        || card.name.len() > 100
        || !card
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err("回顾文件名无效。".into());
    }
    if card.png.len() > MAX_PNG * 4 / 3 + 4 {
        return Err("回顾图片超过大小上限。".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&card.png)
        .map_err(|_| "回顾图片无效。")?;
    let mut decoder = png::Decoder::new(Cursor::new(&bytes));
    decoder.set_limits(png::Limits {
        bytes: 16 * 1024 * 1024,
    });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| "回顾 PNG 无效。")?;
    if reader.info().width != 1080 || reader.info().height != 1920 {
        return Err("回顾图片必须是 1080 × 1920。".into());
    }
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| "回顾 PNG 解码失败。")?;
    pixels.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels,
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => return Err("回顾图片颜色格式无效。".into()),
    };
    Ok((bytes, rgba))
}
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::credential_store::atomic_write_bounded(
        path,
        bytes,
        std::fs::read(path).ok().as_deref(),
        MAX_PNG as u64,
    )
    .map_err(|_| "无法保存回顾图片，请检查目标目录。".into())
}
#[tauri::command]
pub async fn recap_export(cards: Vec<Card>, action: String) -> Result<String, String> {
    if cards.is_empty()
        || cards.len() > 8
        || !matches!(action.as_str(), "save" | "saveAll" | "copy" | "smoke")
    {
        return Err("回顾导出参数无效。".into());
    }
    let names: std::collections::HashSet<_> = cards.iter().map(|c| c.name.as_str()).collect();
    if names.len() != cards.len() {
        return Err("回顾图片文件名重复。".into());
    }
    if action != "saveAll" && cards.len() != 1 {
        return Err("此操作只能选择一张回顾图片。".into());
    }
    tokio::task::spawn_blocking(move || {
        let decoded = cards.iter().map(decode).collect::<Result<Vec<_>, _>>()?;
        if action == "copy" {
            arboard::Clipboard::new()
                .and_then(|mut c| {
                    c.set_image(arboard::ImageData {
                        width: 1080,
                        height: 1920,
                        bytes: Cow::Borrowed(&decoded[0].1),
                    })
                })
                .map_err(|_| "无法复制图片，请稍后重试。")?;
            return Ok("copied".into());
        }
        if action == "smoke" {
            if !std::env::args().any(|a| a == "--native-smoke") {
                return Err("此操作仅供隔离验收。".into());
            }
            let path = crate::settings::directory()
                .ok_or("无法定位验收目录。")?
                .join(&cards[0].name);
            write(&path, &decoded[0].0)?;
            return Ok("saved".into());
        }
        if action == "saveAll" {
            let Some(folder) = rfd::FileDialog::new()
                .set_title("保存全部回顾卡片")
                .pick_folder()
            else {
                return Ok("cancelled".into());
            };
            for (card, (png, _)) in cards.iter().zip(&decoded) {
                write(&folder.join(&card.name), png)?
            }
            return Ok("saved".into());
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("保存回顾卡片")
            .add_filter("PNG 图片", &["png"])
            .set_file_name(&cards[0].name)
            .save_file()
        else {
            return Ok("cancelled".into());
        };
        write(&path, &decoded[0].0)?;
        Ok("saved".into())
    })
    .await
    .map_err(|_| "回顾导出任务失败。".to_owned())?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_paths_and_images_are_bounded() {
        for name in ["../x.png", "pulse-recap-../x.png", "C:/x.png", "x.png"] {
            assert!(decode(&Card {
                name: name.into(),
                png: "".into()
            })
            .is_err())
        }
        assert!(decode(&Card {
            name: "pulse-recap-2026-01-poster.png".into(),
            png: "invalid".into()
        })
        .is_err())
    }
    #[test]
    fn png_round_trip_has_exact_export_dimensions() {
        let mut bytes = vec![];
        {
            let mut enc = png::Encoder::new(&mut bytes, 1080, 1920);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![255; 1080 * 1920 * 4])
                .unwrap();
        }
        let card = Card {
            name: "pulse-recap-2026-01-poster.png".into(),
            png: base64::engine::general_purpose::STANDARD.encode(&bytes),
        };
        let (png, pixels) = decode(&card).unwrap();
        assert_eq!(png, bytes);
        assert_eq!(pixels.len(), 1080 * 1920 * 4);
        assert!(pixels.iter().all(|b| *b == 255))
    }
}
