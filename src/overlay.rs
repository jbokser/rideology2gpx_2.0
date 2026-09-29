use std::{
    ffi::OsString,
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{Result, Sample};

const LEADING_ZERO_COLOR: [u8; 3] = [43, 51, 58];
const MAX_COLOR: [u8; 3] = [150, 35, 45];

pub struct Options {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub redline_rpm: f64,
    pub temp_warning_c: f64,
}

pub fn path(input: &Path, directory: &Path, trip: Option<usize>) -> Result<PathBuf> {
    let mut name = OsString::from(input.file_stem().ok_or("Input path has no file stem")?);
    name.push(trip.map_or_else(|| ".mp4".to_owned(), |trip| format!("-trip-{trip}.mp4")));
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
        'B' => [30, 17, 17, 30, 17, 17, 30],
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
        'X' => [17, 17, 10, 4, 10, 17, 17],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        'º' => [6, 9, 6, 0, 0, 0, 0],
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
    fn outline_rect(&mut self, x: i32, y: i32, w: i32, h: i32, thickness: i32, color: [u8; 3]) {
        self.rect(x, y, w, thickness, color);
        self.rect(x, y + h - thickness, w, thickness, color);
        self.rect(x, y, thickness, h, color);
        self.rect(x + w - thickness, y, thickness, h, color);
    }
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 3]) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for step in 0..=steps {
            self.rect(
                x0 + (x1 - x0) * step / steps - 1,
                y0 + (y1 - y0) * step / steps - 1,
                3,
                3,
                color,
            );
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

fn fixed_number(
    canvas: &mut Canvas,
    value: f64,
    digits: usize,
    x: i32,
    y: i32,
    scale: i32,
    color: [u8; 3],
) {
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
                LEADING_ZERO_COLOR
            } else {
                color
            },
        );
    }
}

fn heading(samples: &[Sample], index: usize) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    for i in (0..=index.min(samples.len() - 2))
        .rev()
        .chain(index + 1..samples.len() - 1)
    {
        let a = &samples[i];
        let b = &samples[i + 1];
        let lat1 = a.lat.to_radians();
        let lat2 = b.lat.to_radians();
        let dlon = (b.lon - a.lon).to_radians();
        let east = dlon.sin() * lat2.cos();
        let north = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
        if east.abs() + north.abs() > 1e-12 {
            return Some(east.atan2(north).to_degrees().rem_euclid(360.0));
        }
    }
    None
}

fn gear_number(gear: &str) -> Option<i32> {
    if gear == "N" {
        Some(0)
    } else {
        gear.parse().ok()
    }
}

fn compass_needle(canvas: &mut Canvas, cx: i32, cy: i32, degrees: f64, scale: f64, color: [u8; 3]) {
    let angle = degrees.to_radians();
    let forward = (angle.sin(), -angle.cos());
    let side = (angle.cos(), angle.sin());
    let point = |along: f64, across: f64| -> (i32, i32) {
        (
            (cx as f64 + (forward.0 * along + side.0 * across) * scale).round() as i32,
            (cy as f64 + (forward.1 * along + side.1 * across) * scale).round() as i32,
        )
    };
    let tip = point(10.0, 0.0);
    let left = point(4.0, -4.0);
    let right = point(4.0, 4.0);
    let tail = point(-9.0, 0.0);
    canvas.line(tail.0, tail.1, tip.0, tip.1, color);
    canvas.line(left.0, left.1, tip.0, tip.1, color);
    canvas.line(right.0, right.1, tip.0, tip.1, color);
}

fn shift_triangle(canvas: &mut Canvas, x: i32, y: i32, up: bool, scale: f64, color: [u8; 3]) {
    let height = (30.0 * scale).round().max(1.0) as i32;
    let half_width = (16.0 * scale).round().max(1.0) as i32;
    for row in 0..height {
        let spread = half_width * row / height;
        let yy = if up {
            y - height / 2 + row
        } else {
            y + height / 2 - row
        };
        canvas.rect(x - spread, yy, 2 * spread + 1, 1, color);
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn shift_indicator(samples: &[Sample], index: usize, at: f64) -> Option<bool> {
    let current = samples.get(index)?;
    let next = samples.get(index + 1)?;
    let remaining = next.ms - at;
    if !(0.0 < remaining && remaining <= 1000.0) {
        return None;
    }
    let before = gear_number(&current.gear)?;
    let after = gear_number(&next.gear)?;
    let elapsed = 1000.0 - remaining;
    (before != after && ((elapsed / 125.0).floor() as u32).is_multiple_of(2))
        .then_some(after > before)
}

fn peak_sample(samples: &[Sample], value: impl Fn(&Sample) -> f64) -> Option<&Sample> {
    samples
        .iter()
        .filter(|sample| value(sample) > 0.0)
        .max_by(|a, b| {
            value(a)
                .total_cmp(&value(b))
                .then_with(|| b.ms.total_cmp(&a.ms))
        })
}

fn peak_visible(peak: Option<&Sample>, at: f64) -> bool {
    peak.is_some_and(|peak| at >= peak.ms && at < peak.ms + 2000.0)
}

fn preview_start_ms(samples: &[Sample]) -> Option<f64> {
    let first = samples.first()?;
    let last = samples.last()?;
    let duration_ms = last.ms - first.ms;
    if duration_ms <= 30_000.0 {
        return None;
    }
    let peak = samples
        .iter()
        .max_by(|a, b| a.rpm.total_cmp(&b.rpm).then_with(|| b.ms.total_cmp(&a.ms)))?;
    Some((peak.ms - first.ms - 20_000.0).clamp(0.0, duration_ms - 30_000.0))
}

pub fn write(path: &Path, samples: &[Sample], options: &Options) -> Result<()> {
    if samples.is_empty() {
        return Err("Cannot render an empty trip".into());
    }
    let first_ms = samples[0].ms;
    let duration_ms = samples.last().unwrap().ms - first_ms;
    let display_duration_ms = [
        peak_sample(samples, |sample| sample.rpm),
        peak_sample(samples, |sample| sample.speed),
    ]
    .into_iter()
    .flatten()
    .map(|peak| peak.ms - first_ms + 2000.0)
    .fold(duration_ms, f64::max);
    let full_frames = (display_duration_ms / 1000.0 * options.fps as f64).ceil() as u64 + 1;
    if full_frames > 10_000_000 {
        return Err("Overlay duration is too long".into());
    }
    let stem = path
        .file_stem()
        .ok_or("Video path has no file stem")?
        .to_string_lossy();
    let preview_image = path.with_file_name(format!("{stem}-preview.jpg"));
    let preview_video = path.with_file_name(format!("{stem}-preview.mp4"));
    if let Some(start_ms) = preview_start_ms(samples) {
        render(
            &preview_video,
            samples,
            options,
            start_ms,
            30 * options.fps as u64,
            None,
        )?;
        eprintln!("Preview video saved to {}", preview_video.display());
    } else if preview_video.exists() {
        fs::remove_file(&preview_video)?;
    }
    render(
        path,
        samples,
        options,
        0.0,
        full_frames,
        Some(&preview_image),
    )
}

fn render(
    path: &Path,
    samples: &[Sample],
    options: &Options,
    start_ms: f64,
    frames: u64,
    preview_image: Option<&Path>,
) -> Result<()> {
    let duration_ms = samples.last().unwrap().ms - samples[0].ms;
    let preview_frame = (frames - 1) / 2;
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
    eprintln!(
        "Rendering {} video...",
        if preview_image.is_some() {
            "overlay"
        } else {
            "preview"
        }
    );
    let show_progress = io::stderr().is_terminal();
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
    base.text("TEMP ºC", x(345.0), y(177.0), size(2.0).max(1), dim);
    base.text("HEADING", x(650.0), y(177.0), size(2.0).max(1), dim);
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
    let bar_height = size(26.0);
    let actual_bar_width = (bars - 1) * (segment_width + gap) + segment_width;
    let border_padding = size(3.0).max(3);
    let border_thickness = size(2.0).max(2);
    let rpm_peak = peak_sample(samples, |sample| sample.rpm);
    let speed_peak = peak_sample(samples, |sample| sample.speed);
    let rpm_peak_label = rpm_peak.map(|sample| format!("MAX {}", sample.rpm.round() as u64));
    let speed_peak_label = speed_peak.map(|sample| format!("MAX {}", sample.speed.round() as u64));
    for frame in 0..frames {
        let timeline_ms = start_ms + frame as f64 * 1000.0 / options.fps as f64;
        let t = timeline_ms.min(duration_ms);
        let at = samples[0].ms + t;
        let display_at = samples[0].ms + timeline_ms;
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
        let shift = shift_indicator(samples, index, at);
        let mut c = base.clone();
        if peak_visible(speed_peak, display_at) {
            c.text(
                speed_peak_label.as_deref().unwrap(),
                x(130.0),
                y(190.0),
                size(3.0).max(1),
                MAX_COLOR,
            );
        }
        if peak_visible(rpm_peak, display_at) {
            c.text(
                rpm_peak_label.as_deref().unwrap(),
                x(420.0),
                y(18.0),
                size(3.0).max(1),
                MAX_COLOR,
            );
        }
        fixed_number(&mut c, speed, 3, x(40.0), y(72.0), size(14.0).max(1), white);
        let rpm_color = if rpm > options.redline_rpm && (t / 250.0).floor() as u64 % 2 == 1 {
            [255, 145, 30]
        } else {
            white
        };
        fixed_number(
            &mut c,
            rpm,
            5,
            x(345.0),
            y(52.0),
            size(7.0).max(1),
            rpm_color,
        );
        let temp_color = if temp > options.temp_warning_c {
            yellow
        } else {
            white
        };
        fixed_number(
            &mut c,
            temp,
            3,
            x(345.0),
            y(202.0),
            size(4.0).max(1),
            temp_color,
        );
        let interval = if next > index {
            index
        } else {
            index.saturating_sub(1)
        };
        let acceleration = if samples.len() > 1 {
            (samples[interval + 1].speed - samples[interval].speed)
                / 3.6
                / ((samples[interval + 1].ms - samples[interval].ms) / 1000.0)
                / 9.80665
        } else {
            0.0
        };
        c.text(
            if acceleration < 0.0 {
                "BRAKE G"
            } else {
                "ACCEL G"
            },
            x(485.0),
            y(177.0),
            size(2.0).max(1),
            dim,
        );
        c.text(
            &format!("{acceleration:+.2}"),
            x(485.0),
            y(202.0),
            size(3.0).max(1),
            white,
        );
        if let Some(degrees) = heading(samples, index) {
            let compass = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
            let label = compass[((degrees / 45.0 + 0.5).floor() as usize) % 8];
            let direction = format!("{label} {:03.0}º", degrees.round() % 360.0);
            c.text(&direction, x(690.0), y(202.0), size(3.0).max(1), white);
            compass_needle(&mut c, x(663.0), y(212.0), degrees, scale, white);
        }
        c.rect(
            bar_x - border_padding,
            bar_y - border_padding,
            actual_bar_width + 2 * border_padding,
            bar_height + 2 * border_padding,
            LEADING_ZERO_COLOR,
        );
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
                bar_height,
                color,
            );
        }
        c.outline_rect(
            bar_x - border_padding,
            bar_y - border_padding,
            actual_bar_width + 2 * border_padding,
            bar_height + 2 * border_padding,
            border_thickness,
            [255, 255, 255],
        );
        if let Some(up) = shift {
            shift_triangle(&mut c, x(863.0), y(82.0), up, scale, white);
        }
        c.text(
            gear,
            x(760.0),
            y(53.0),
            size(9.0).max(1),
            if gear == "N" { green } else { white },
        );
        if frame == preview_frame
            && let Some(preview_path) = preview_image
        {
            image::save_buffer_with_format(
                preview_path,
                &c.data,
                options.width,
                options.height,
                image::ColorType::Rgb8,
                image::ImageFormat::Jpeg,
            )?;
        }
        stdin
            .write_all(&c.data)
            .map_err(|e| format!("Cannot write video frame: {e}"))?;
        let progress = (frame + 1) * 100 / frames;
        let previous = frame * 100 / frames;
        if show_progress && (frame == 0 || progress / 5 > previous / 5) {
            let filled = (progress / 5) as usize;
            eprint!(
                "\r[{}{}] {:3}%",
                "#".repeat(filled),
                ".".repeat(20 - filled),
                progress
            );
            io::stderr().flush()?;
        }
    }
    drop(stdin);
    let status = child.wait()?;
    if show_progress {
        eprint!("\r\x1b[2K");
        io::stderr().flush()?;
    }
    if !status.success() {
        return Err(format!("ffmpeg failed with {status}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: f64, rpm: f64) -> Sample {
        Sample {
            ms,
            rpm,
            lat: 0.0,
            lon: 0.0,
            speed: 10.0,
            temp: 90.0,
            gear: "1".into(),
            area: None,
        }
    }

    #[test]
    fn peak_labels_start_at_first_maximum_and_last_two_seconds() {
        let mut samples = [
            sample(0.0, 1000.0),
            sample(1000.0, 5000.0),
            sample(2000.0, 5000.0),
        ];
        samples[2].speed = 80.0;
        let rpm_peak = peak_sample(&samples, |sample| sample.rpm);
        let speed_peak = peak_sample(&samples, |sample| sample.speed);
        assert_eq!(
            rpm_peak.map(|sample| (sample.ms, sample.rpm)),
            Some((1000.0, 5000.0))
        );
        assert_eq!(
            speed_peak.map(|sample| (sample.ms, sample.speed)),
            Some((2000.0, 80.0))
        );
        assert!(!peak_visible(rpm_peak, 999.0));
        assert!(peak_visible(rpm_peak, 1000.0));
        assert!(peak_visible(rpm_peak, 2999.0));
        assert!(!peak_visible(rpm_peak, 3000.0));
        assert!(!peak_visible(None, 1000.0));
    }

    #[test]
    fn shift_indicator_blinks_during_second_before_change() {
        let mut samples = [sample(0.0, 1000.0), sample(2000.0, 2000.0)];
        samples[1].gear = "2".into();
        assert_eq!(shift_indicator(&samples, 0, 999.0), None);
        assert_eq!(shift_indicator(&samples, 0, 1000.0), Some(true));
        assert_eq!(shift_indicator(&samples, 0, 1125.0), None);
        assert_eq!(shift_indicator(&samples, 0, 1250.0), Some(true));
        assert_eq!(shift_indicator(&samples, 1, 2000.0), None);
        samples[1].gear = "N".into();
        assert_eq!(shift_indicator(&samples, 0, 1000.0), Some(false));
    }

    #[test]
    fn preview_window_uses_peak_and_stays_inside_trip() {
        assert_eq!(
            preview_start_ms(&[sample(0.0, 1000.0), sample(30_000.0, 9000.0)]),
            None
        );
        assert_eq!(
            preview_start_ms(&[sample(0.0, 9000.0), sample(35_000.0, 1000.0)]),
            Some(0.0)
        );
        assert_eq!(
            preview_start_ms(&[
                sample(0.0, 1000.0),
                sample(25_000.0, 9000.0),
                sample(35_000.0, 1000.0)
            ]),
            Some(5000.0)
        );
        assert_eq!(
            preview_start_ms(&[
                sample(0.0, 1000.0),
                sample(55_000.0, 9000.0),
                sample(60_000.0, 1000.0)
            ]),
            Some(30_000.0)
        );
    }
}
