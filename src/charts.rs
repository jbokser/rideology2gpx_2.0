use std::{
    ffi::OsString,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use plotters::{
    prelude::*,
    style::text_anchor::{HPos, Pos, VPos},
};

use crate::{Result, Sample, distance, median};

pub const SIZE: (u32, u32) = (1400, 1100);

pub fn chart_path(input: &Path, output_directory: &Path, trip: usize) -> Result<PathBuf> {
    let mut name = OsString::from(input.file_stem().ok_or("Input path has no file stem")?);
    name.push(format!("-trip-{trip}.jpg"));
    Ok(output_directory.join(name))
}

fn speed_by_distance(samples: &[Sample]) -> Vec<(f64, f64)> {
    let intervals: Vec<_> = samples
        .windows(2)
        .map(|w| (w[1].ms - w[0].ms) / 1000.0)
        .collect();
    let nominal = if intervals.is_empty() {
        1.0
    } else {
        median(&intervals)
    };
    let mut cumulative_km = 0.0;
    samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            if i > 0 && intervals[i - 1] <= nominal * 1.5 {
                cumulative_km += distance(&samples[i - 1], sample) / 1000.0;
            }
            (cumulative_km, sample.speed)
        })
        .collect()
}

fn chart_title(samples: &[Sample], date: &str) -> String {
    let start = samples
        .first()
        .and_then(|s| s.area.as_deref())
        .unwrap_or("Unknown start");
    let end = samples
        .last()
        .and_then(|s| s.area.as_deref())
        .unwrap_or("Unknown end");
    format!("{start} → {end} ({date})")
}

fn axis_label(value: f64) -> String {
    format!("{value:.2}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn gear_steps(samples: &[Sample], distances: &[(f64, f64)]) -> Vec<Vec<(f64, i32)>> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    let mut previous = None;
    for (sample, (x, _)) in samples.iter().zip(distances) {
        let gear = match sample.gear.as_str() {
            "N" => Some(0),
            value => value.parse::<i32>().ok().filter(|v| (1..=6).contains(v)),
        };
        if let Some(gear) = gear {
            if let Some(previous) = previous {
                current.push((*x, previous));
            }
            current.push((*x, gear));
            previous = Some(gear);
        } else {
            if !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            previous = None;
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

pub fn write_trip_chart(path: &Path, samples: &[Sample], date: &str) -> Result<()> {
    if samples.is_empty() {
        return Err("Cannot chart an empty trip".into());
    }
    let points = speed_by_distance(samples);
    let total_km = points.last().unwrap().0;
    let max_speed = samples.iter().map(|s| s.speed).fold(0.0_f64, f64::max);
    let mut pixels = vec![255_u8; (SIZE.0 * SIZE.1 * 3) as usize];
    {
        let root = BitMapBackend::with_buffer(&mut pixels, SIZE).into_drawing_area();
        root.fill(&WHITE)?;
        let panels = root.titled(&chart_title(samples, date), ("sans-serif", 32))?;
        let (speed_panel, lower_panels) = panels.split_vertically(470);
        let (rpm_panel, gear_panel) = lower_panels.split_vertically(280);
        let x_max = (total_km * 1.02).max(0.1);
        let mut chart = ChartBuilder::on(&speed_panel)
            .margin_left(35)
            .margin_right(35)
            .margin_top(20)
            .margin_bottom(15)
            .x_label_area_size(0)
            .y_label_area_size(80)
            .build_cartesian_2d(
                0.0..x_max,
                0.0..((max_speed * 1.25 / 20.0).ceil() * 20.0).max(20.0),
            )?;
        chart
            .configure_mesh()
            .y_desc("Speed (km/h)")
            .axis_desc_style(("sans-serif", 24))
            .label_style(("sans-serif", 19))
            .x_labels(10)
            .y_labels(10)
            .x_label_formatter(&|_| String::new())
            .y_label_formatter(&|value| format!("{value:.0}"))
            .light_line_style(RGBColor(235, 239, 244))
            .bold_line_style(RGBColor(215, 222, 230))
            .draw()?;
        let blue = RGBColor(28, 107, 174);
        chart.draw_series(LineSeries::new(
            points.iter().copied(),
            blue.stroke_width(2),
        ))?;
        if points.len() == 1 {
            chart.draw_series(std::iter::once(Circle::new(points[0], 5, blue.filled())))?;
        }
        let max_rpm = samples.iter().map(|s| s.rpm).fold(0.0_f64, f64::max);
        let rpm_points: Vec<_> = points
            .iter()
            .zip(samples)
            .map(|((x, _), s)| (*x, s.rpm))
            .collect();
        let mut rpm_chart = ChartBuilder::on(&rpm_panel)
            .margin_left(35)
            .margin_right(35)
            .margin_top(10)
            .margin_bottom(15)
            .x_label_area_size(0)
            .y_label_area_size(80)
            .build_cartesian_2d(
                0.0..x_max,
                0.0..((max_rpm * 1.4 / 1000.0).ceil() * 1000.0).max(1000.0),
            )?;
        rpm_chart
            .configure_mesh()
            .y_desc("Engine speed (rpm)")
            .axis_desc_style(("sans-serif", 24))
            .label_style(("sans-serif", 19))
            .x_labels(10)
            .y_labels(5)
            .x_label_formatter(&|_| String::new())
            .y_label_formatter(&|v| {
                if *v == 0.0 {
                    "0".into()
                } else {
                    format!("{}k", axis_label(*v / 1000.0))
                }
            })
            .light_line_style(RGBColor(235, 239, 244))
            .bold_line_style(RGBColor(215, 222, 230))
            .draw()?;
        let purple = RGBColor(120, 75, 165);
        rpm_chart.draw_series(LineSeries::new(
            rpm_points.iter().copied(),
            purple.stroke_width(2),
        ))?;
        if rpm_points.len() == 1 {
            rpm_chart.draw_series(std::iter::once(Circle::new(
                rpm_points[0],
                4,
                purple.filled(),
            )))?;
        }
        let mut gear_chart = ChartBuilder::on(&gear_panel)
            .margin_left(35)
            .margin_right(35)
            .margin_top(10)
            .margin_bottom(25)
            .x_label_area_size(60)
            .y_label_area_size(80)
            .build_cartesian_2d(0.0..x_max, -1..7)?;
        gear_chart
            .configure_mesh()
            .x_desc("Distance traveled (km)")
            .y_desc("Gear")
            .axis_desc_style(("sans-serif", 24))
            .label_style(("sans-serif", 19))
            .x_labels(10)
            .y_labels(9)
            .x_label_formatter(&|value| axis_label(*value))
            .y_label_formatter(&|value| match value {
                0 => "N".into(),
                1..=6 => value.to_string(),
                _ => String::new(),
            })
            .disable_y_mesh()
            .bold_line_style(RGBColor(215, 222, 230))
            .draw()?;
        let green = RGBColor(33, 135, 98);
        for segment in gear_steps(samples, &points) {
            if segment.len() == 1 {
                gear_chart.draw_series(std::iter::once(Circle::new(
                    segment[0],
                    4,
                    green.filled(),
                )))?;
            } else {
                gear_chart.draw_series(LineSeries::new(segment, green.stroke_width(2)))?;
            }
        }
        // Mark the first occurrence of each maximum with a shared annotation style.
        let speed_peak = points.iter().find(|point| point.1 == max_speed).unwrap();
        let rpm_peak = rpm_points.iter().find(|point| point.1 == max_rpm).unwrap();
        for ((px, py), value, unit) in [
            (
                chart.plotting_area().map_coordinate(speed_peak),
                max_speed,
                "km/h",
            ),
            (
                rpm_chart.plotting_area().map_coordinate(rpm_peak),
                max_rpm,
                "rpm",
            ),
        ] {
            let offset_x = if px > SIZE.0 as i32 - 220 { -28 } else { 28 };
            let label_x = (px + offset_x).clamp(210, SIZE.0 as i32 - 160);
            let label_y = py - 42;
            let red = RGBColor(180, 42, 38);
            let tip = (px as f64, py as f64 - 5.0);
            let origin = ((px + offset_x) as f64, py as f64 - 34.0);
            let length = ((tip.0 - origin.0).powi(2) + (tip.1 - origin.1).powi(2)).sqrt();
            let direction = ((tip.0 - origin.0) / length, (tip.1 - origin.1) / length);
            let base = (tip.0 - 8.0 * direction.0, tip.1 - 8.0 * direction.1);
            root.draw(&PathElement::new(
                vec![
                    (origin.0 as i32, origin.1 as i32),
                    (tip.0 as i32, tip.1 as i32),
                ],
                BLACK.stroke_width(2),
            ))?;
            root.draw(&Polygon::new(
                vec![
                    (tip.0 as i32, tip.1 as i32),
                    (
                        (base.0 - 3.0 * direction.1) as i32,
                        (base.1 + 3.0 * direction.0) as i32,
                    ),
                    (
                        (base.0 + 3.0 * direction.1) as i32,
                        (base.1 - 3.0 * direction.0) as i32,
                    ),
                ],
                BLACK.filled(),
            ))?;
            root.draw(&Circle::new((px, py), 4, red.filled()))?;
            root.draw(&Text::new(
                format!("Max: {} {unit}", axis_label(value)),
                (label_x, label_y),
                ("sans-serif", 23)
                    .into_font()
                    .color(&BLACK)
                    .pos(Pos::new(HPos::Center, VPos::Bottom)),
            ))?;
        }
        root.present()?;
    }
    let mut file = BufWriter::new(File::create(path)?);
    JpegEncoder::new_with_quality(&mut file, 92).encode(
        &pixels,
        SIZE.0,
        SIZE.1,
        ExtendedColorType::Rgb8,
    )?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: f64, lon: f64, speed: f64) -> Sample {
        Sample {
            ms,
            lat: 0.0,
            lon,
            rpm: 1000.0,
            speed,
            temp: 90.0,
            gear: "1".into(),
            area: None,
        }
    }

    #[test]
    fn gear_changes_are_steps_and_unknown_gears_break_the_line() {
        let mut samples = vec![sample(0.0, 0.0, 10.0); 5];
        for (s, gear) in samples.iter_mut().zip(["N", "2", "?", "6", "5"]) {
            s.gear = gear.into();
        }
        let distances = vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (3.0, 0.0), (4.0, 0.0)];
        assert_eq!(
            gear_steps(&samples, &distances),
            vec![
                vec![(0.0, 0), (1.0, 0), (1.0, 2)],
                vec![(3.0, 6), (4.0, 6), (4.0, 5)]
            ]
        );
    }

    #[test]
    fn labels_preserve_fractional_distances_without_trailing_zeros() {
        assert_eq!(axis_label(20.0), "20");
        assert_eq!(axis_label(0.0), "0");
        assert_eq!(axis_label(0.2), "0.2");
        let mut samples = vec![sample(0.0, 0.0, 10.0), sample(1000.0, 0.001, 20.0)];
        samples[0].area = Some("Florida".into());
        samples[1].area = Some("Tigre".into());
        assert_eq!(
            chart_title(&samples, "2026-09-22"),
            "Florida → Tigre (2026-09-22)"
        );
    }

    #[test]
    fn distance_is_cumulative_in_kilometers_and_ignores_gaps() {
        let samples = [
            sample(0.0, 0.0, 10.0),
            sample(1000.0, 0.001, 20.0),
            sample(2000.0, 0.002, 0.0),
            sample(100000.0, 1.0, 30.0),
        ];
        let points = speed_by_distance(&samples);
        assert_eq!(points[0], (0.0, 10.0));
        assert!((points[1].0 - 0.111195).abs() < 0.000001);
        assert!((points[2].0 - 0.22239).abs() < 0.000001);
        assert_eq!(points[2].1, 0.0);
        assert_eq!(points[2].0, points[3].0);
        assert_eq!(speed_by_distance(&samples[1..2]), vec![(0.0, 20.0)]);
    }
}
