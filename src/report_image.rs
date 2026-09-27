use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use plotters::{
    prelude::*,
    style::text_anchor::{HPos, Pos, VPos},
};

use crate::Result;

const WIDTH: u32 = 1400;
const LEFT: i32 = 80;
const RIGHT: i32 = 1320;

fn plain(value: &str) -> String {
    value
        .replace("\\|", "|")
        .replace("\\#", "#")
        .replace("\\_", "_")
        .replace("\\*", "*")
        .replace("\\`", "`")
        .replace("\\[", "[")
        .replace("\\]", "]")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("\\\\", "\\")
}

fn rows(markdown: &str) -> Vec<(String, Vec<String>)> {
    markdown
        .lines()
        .filter_map(|line| {
            if let Some(title) = line.strip_prefix("# ") {
                Some(("title".into(), vec![plain(title)]))
            } else if let Some(heading) = line.strip_prefix("## ") {
                Some(("heading".into(), vec![plain(heading)]))
            } else if line.starts_with('|') {
                let cells: Vec<_> = line
                    .trim_matches('|')
                    .split(" | ")
                    .map(|cell| plain(cell.trim()))
                    .collect();
                if cells.iter().all(|cell| cell.starts_with("---")) {
                    None
                } else {
                    Some(("table".into(), cells))
                }
            } else if line.trim().is_empty() {
                None
            } else {
                Some(("paragraph".into(), vec![plain(line)]))
            }
        })
        .collect()
}

pub fn write(path: &Path, markdown: &str) -> Result<()> {
    let rows = rows(markdown);
    let height: u32 = 110
        + rows
            .iter()
            .map(|(kind, _)| match kind.as_str() {
                "title" => 85,
                "heading" => 72,
                "table" => 50,
                _ => 57,
            })
            .sum::<u32>()
        + 50;
    let mut pixels = vec![255_u8; (WIDTH * height * 3) as usize];
    {
        let root = BitMapBackend::with_buffer(&mut pixels, (WIDTH, height)).into_drawing_area();
        root.fill(&WHITE)?;
        let mut y = 85;
        for (kind, cells) in rows {
            match kind.as_str() {
                "title" => {
                    root.draw(&Text::new(
                        cells[0].clone(),
                        (LEFT, y),
                        ("sans-serif", 38)
                            .into_font()
                            .color(&BLACK)
                            .pos(Pos::new(HPos::Left, VPos::Center)),
                    ))?;
                    y += 85;
                }
                "heading" => {
                    root.draw(&PathElement::new(
                        vec![(LEFT, y - 22), (RIGHT, y - 22)],
                        RGBColor(210, 218, 226).stroke_width(2),
                    ))?;
                    root.draw(&Text::new(
                        cells[0].clone(),
                        (LEFT, y + 8),
                        ("sans-serif", 29)
                            .into_font()
                            .color(&RGBColor(28, 70, 108))
                            .pos(Pos::new(HPos::Left, VPos::Center)),
                    ))?;
                    y += 72;
                }
                "table" => {
                    let header = cells
                        .first()
                        .is_some_and(|cell| cell == "Metric" || cell == "Gear");
                    if header {
                        root.draw(&Rectangle::new(
                            [(LEFT, y - 23), (RIGHT, y + 25)],
                            RGBColor(232, 239, 245).filled(),
                        ))?;
                    }
                    let positions: &[i32] = if cells.len() == 3 {
                        &[LEFT + 15, 650, 1000]
                    } else {
                        &[LEFT + 15, 680]
                    };
                    for (cell, &x) in cells.iter().zip(positions) {
                        root.draw(&Text::new(
                            cell.clone(),
                            (x, y),
                            ("sans-serif", 21)
                                .into_font()
                                .color(&BLACK)
                                .pos(Pos::new(HPos::Left, VPos::Center)),
                        ))?;
                    }
                    root.draw(&PathElement::new(
                        vec![(LEFT, y + 25), (RIGHT, y + 25)],
                        RGBColor(225, 229, 233).stroke_width(1),
                    ))?;
                    y += 50;
                }
                _ => {
                    root.draw(&Text::new(
                        cells[0].clone(),
                        (LEFT, y),
                        ("sans-serif", 23)
                            .into_font()
                            .color(&BLACK)
                            .pos(Pos::new(HPos::Left, VPos::Center)),
                    ))?;
                    y += 57;
                }
            }
        }
        root.present()?;
    }
    let mut file = BufWriter::new(File::create(path)?);
    JpegEncoder::new_with_quality(&mut file, 92).encode(
        &pixels,
        WIDTH,
        height,
        ExtendedColorType::Rgb8,
    )?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_rows_keep_report_content() {
        let parsed = rows(
            "# A &amp; B\n\n## Trip 1\n\n| Metric | Value |\n| --- | --- |\n| Speed | 20 km/h |\n",
        );
        assert_eq!(parsed[0].1[0], "A & B");
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[3].1, vec!["Speed", "20 km/h"]);
    }
}
