use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{Result, Sample};

pub struct Options {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub redline_rpm: f64,
}

pub fn path(input: &Path, directory: &Path, trip: usize) -> Result<PathBuf> {
    let mut name = OsString::from(input.file_stem().ok_or("Input path has no file stem")?);
    name.push(format!("-trip-{trip}.mp4"));
    Ok(directory.join(name))
}

fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'C' => [15, 16, 16, 16, 16, 16, 15],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'G' => [15, 16, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        _ => [0; 7],
    }
}

#[derive(Clone)]
struct Canvas {
    data: Vec<u8>,
    w: i32,
    h: i32,
}
impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self {
            data: vec![0; w as usize * h as usize * 3],
            w: w as i32,
            h: h as i32,
        }
    }
    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: [u8; 3]) {
        for yy in y.max(0)..(y + h).min(self.h) {
            for xx in x.max(0)..(x + w).min(self.w) {
                let i = ((yy * self.w + xx) * 3) as usize;
                self.data[i..i + 3].copy_from_slice(&color);
            }
        }
    }
    fn text(&mut self, value: &str, x: i32, y: i32, scale: i32, color: [u8; 3]) {
        for (n, ch) in value.chars().enumerate() {
            for (row, bits) in glyph(ch.to_ascii_uppercase()).iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        self.rect(
                            x + (n as i32 * 6 + col) * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
        }
    }
}

fn fixed_number(canvas: &mut Canvas, value: f64, digits: usize, x: i32, y: i32, scale: i32) {
    let rounded = value.max(0.0).round() as u64;
    let text = format!("{rounded:0digits$}");
    let leading = text
        .bytes()
        .take_while(|digit| *digit == b'0')
        .count()
        .min(text.len().saturating_sub(1));
    for (index, digit) in text.chars().enumerate() {
        canvas.text(
            &digit.to_string(),
            x + index as i32 * 6 * scale,
            y,
            scale,
            if index < leading {
                [55, 65, 74]
            } else {
                [245, 250, 255]
            },
        );
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn write(path: &Path, samples: &[Sample], options: &Options) -> Result<()> {
    if samples.is_empty() {
        return Err("Cannot render an empty trip".into());
    }
    let duration = (samples.last().unwrap().ms - samples[0].ms) / 1000.0;
    let frames = (duration * options.fps as f64).ceil() as u64 + 1;
    if frames > 10_000_000 {
        return Err("Overlay duration is too long".into());
    }
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            &format!("{}x{}", options.width, options.height),
            "-framerate",
            &options.fps.to_string(),
            "-i",
            "-",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
        ])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("Cannot open ffmpeg input")?;
    let scale = (options.width as f64 / 960.0).min(options.height as f64 / 256.0);
    let offset_x = (options.width as f64 - 960.0 * scale) / 2.0;
    let offset_y = (options.height as f64 - 256.0 * scale) / 2.0;
    let x = |v: f64| (offset_x + v * scale).round() as i32;
    let y = |v: f64| (offset_y + v * scale).round() as i32;
    let size = |v: f64| (v * scale).round() as i32;
    let white = [245, 250, 255];
    let dim = [174, 194, 210];
    let yellow = [250, 210, 50];
    let red = [248, 58, 65];
    let green = [70, 240, 130];
    let mut base = Canvas::new(options.width, options.height);
    base.text("SPEED", x(40.0), y(18.0), size(3.0).max(1), dim);
    base.text("KM/H", x(40.0), y(190.0), size(3.0).max(1), dim);
    base.text("RPM", x(345.0), y(18.0), size(3.0).max(1), dim);
    base.text("GEAR", x(745.0), y(18.0), size(3.0).max(1), dim);
    base.text("TEMP C", x(345.0), y(177.0), size(2.0).max(1), dim);
    let max_rpm = ((samples
        .iter()
        .map(|s| s.rpm)
        .fold(0.0_f64, f64::max)
        .max(options.redline_rpm + 2000.0)
        .max(12_000.0)
        / 1000.0)
        .ceil())
        * 1000.0;
    let bars = 50;
    let bar_x = x(345.0);
    let bar_y = y(130.0);
    let bar_width = size(570.0);
    let gap = size(2.0).max(1);
    let segment_width = (bar_width - (bars - 1) * gap) / bars;
    for frame in 0..frames {
        let t = (frame as f64 * 1000.0 / options.fps as f64).min(duration * 1000.0);
        let at = samples[0].ms + t;
        let index = samples
            .partition_point(|sample| sample.ms <= at)
            .saturating_sub(1);
        let next = (index + 1).min(samples.len() - 1);
        let fraction = if next == index {
            0.0
        } else {
            (at - samples[index].ms) / (samples[next].ms - samples[index].ms)
        };
        let rpm = lerp(samples[index].rpm, samples[next].rpm, fraction);
        let speed = lerp(samples[index].speed, samples[next].speed, fraction);
        let temp = lerp(samples[index].temp, samples[next].temp, fraction);
        let gear = samples[index].gear.as_str();
        let changed = index > 0 && samples[index - 1].gear != samples[index].gear;
        let highlight = changed && at - samples[index].ms < 1000.0;
        let mut c = base.clone();
        fixed_number(&mut c, speed, 3, x(40.0), y(72.0), size(14.0).max(1));
        fixed_number(&mut c, rpm, 5, x(345.0), y(52.0), size(7.0).max(1));
        fixed_number(&mut c, temp, 3, x(345.0), y(202.0), size(4.0).max(1));
        for bar in 0..bars {
            let lower = bar as f64 * max_rpm / bars as f64;
            let zone = if lower >= options.redline_rpm {
                2
            } else if lower >= options.redline_rpm * 0.8 {
                1
            } else {
                0
            };
            let active = rpm > lower;
            let color = match (zone, active) {
                (2, true) => red,
                (1, true) => yellow,
                (0, true) => green,
                (2, false) => [65, 24, 30],
                (1, false) => [70, 58, 25],
                _ => [24, 56, 40],
            };
            c.rect(
                bar_x + bar * (segment_width + gap),
                bar_y,
                segment_width,
                size(26.0),
                color,
            );
        }
        if highlight {
            c.rect(x(742.0), y(43.0), size(100.0), size(78.0), white);
        }
        c.text(
            gear,
            x(760.0),
            y(53.0),
            size(9.0).max(1),
            if highlight {
                if gear == "N" {
                    [12, 112, 51]
                } else {
                    [12, 94, 158]
                }
            } else if gear == "N" {
                green
            } else {
                white
            },
        );
        stdin
            .write_all(&c.data)
            .map_err(|e| format!("Cannot write video frame: {e}"))?;
        let progress = (frame + 1) * 100 / frames;
        let previous = frame * 100 / frames;
        if frame == 0 || progress / 5 > previous / 5 {
            let filled = (progress / 5) as usize;
            eprint!(
                "\rOverlay {} [{}{}] {:3}%",
                path.display(),
                "#".repeat(filled),
                ".".repeat(20 - filled),
                progress
            );
            io::stderr().flush()?;
        }
    }
    drop(stdin);
    let status = child.wait()?;
    eprintln!("\rOverlay {} [{}] 100%", path.display(), "#".repeat(20));
    if !status.success() {
        return Err(format!("ffmpeg failed with {status}").into());
    }
    Ok(())
}
