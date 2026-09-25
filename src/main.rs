mod charts;
mod geocoding;
mod gpx;
mod overlay;

use std::{collections::BTreeMap, env, error::Error, fs, process};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const EARTH_M: f64 = 6_371_000.0;

#[derive(Clone, Debug)]
struct Sample {
    ms: f64,
    lat: f64,
    lon: f64,
    rpm: f64,
    speed: f64,
    temp: f64,
    gear: String,
    area: Option<String>,
}

fn read_ride(bytes: &[u8]) -> Result<(String, Vec<Sample>)> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.trim_start_matches('\u{feff}').to_owned(),
        Err(_) => {
            let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
            if errors {
                return Err("CSV is neither valid UTF-8 nor Shift-JIS".into());
            }
            text.into_owned()
        }
    };
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut title = "Ride report".to_owned();
    let mut header: Option<csv::StringRecord> = None;
    let mut samples = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let row = record?;
        if header.is_none() {
            if row.get(0) == Some("Title") {
                title = row.get(1).unwrap_or("Ride report").to_owned();
            }
            if row.get(0) == Some("elapsed_msec") {
                header = Some(row);
            }
            continue;
        }
        let headers = header.as_ref().unwrap();
        if row.len() != headers.len() {
            return Err(format!("Record {}: wrong number of columns", index + 1).into());
        }
        let field = |name: &str| -> Result<&str> {
            let col = headers
                .iter()
                .position(|h| {
                    h == name
                        || (name == "water_temperature" && h.starts_with("water_temperature("))
                })
                .ok_or_else(|| format!("Missing column: {name}"))?;
            Ok(row.get(col).unwrap().trim())
        };
        let number = |name: &str| -> Result<f64> {
            let value: f64 = field(name)?
                .parse()
                .map_err(|_| format!("Record {}: invalid {name}", index + 1))?;
            if !value.is_finite() {
                return Err(format!("Record {}: non-finite {name}", index + 1).into());
            }
            Ok(value)
        };
        let sample = Sample {
            ms: number("elapsed_msec")?,
            lat: number("gps_latitude")?,
            lon: number("gps_longitude")?,
            rpm: number("engine_RPM")?,
            speed: number("wheel_speed(km/h)")?,
            temp: number("water_temperature")?,
            gear: field("gear_position")?.to_owned(),
            area: None,
        };
        if sample.ms < 0.0
            || sample.rpm < 0.0
            || sample.speed < 0.0
            || sample.lat.abs() > 90.0
            || sample.lon.abs() > 180.0
        {
            return Err(format!("Record {}: value out of range", index + 1).into());
        }
        if samples
            .last()
            .is_some_and(|previous: &Sample| previous.ms >= sample.ms)
        {
            return Err(format!("Record {}: elapsed time must increase", index + 1).into());
        }
        samples.push(sample);
    }
    if samples.len() < 2 {
        return Err("At least two telemetry samples are required".into());
    }
    Ok((title, samples))
}

fn distance(a: &Sample, b: &Sample) -> f64 {
    let dlat = (b.lat - a.lat).to_radians();
    let dlon = (b.lon - a.lon).to_radians();
    let h = (dlat / 2.0).sin().powi(2)
        + a.lat.to_radians().cos() * b.lat.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_M * h.clamp(0.0, 1.0).sqrt().asin()
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn coordinate(value: f64, positive: char, negative: char) -> String {
    let total = (value.abs() * 360_000.0).round() as u64;
    let degrees = total / 360_000;
    let minutes = total % 360_000 / 6_000;
    let seconds = (total % 6_000) as f64 / 100.0;
    format!(
        "{}{:03}°{:02}′{:05.2}″",
        if value < 0.0 { negative } else { positive },
        degrees,
        minutes,
        seconds
    )
}

fn position(s: &Sample) -> String {
    format!(
        "{} {}{}",
        coordinate(s.lat, 'N', 'S'),
        coordinate(s.lon, 'E', 'W'),
        s.area
            .as_ref()
            .map(|area| format!(" ({area})"))
            .unwrap_or_default()
    )
}

fn heading(title: &str) -> String {
    format!(
        "{title}\n{}\n\n",
        title
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { '=' })
            .collect::<String>()
    )
}

fn report(title: &str, samples: &[Sample], min_speed: f64) -> String {
    heading(title) + &report_body(samples, min_speed) + &gear_report(samples.iter())
}

fn report_body(samples: &[Sample], min_speed: f64) -> String {
    use std::fmt::Write;
    let intervals: Vec<f64> = samples
        .windows(2)
        .map(|w| (w[1].ms - w[0].ms) / 1000.0)
        .collect();
    let nominal = if intervals.is_empty() {
        1.0
    } else {
        median(&intervals)
    };
    // A gap has no observed telemetry, so it contributes no duration or distance.
    let valid: Vec<bool> = intervals.iter().map(|dt| *dt <= nominal * 1.5).collect();
    let segments: Vec<f64> = samples.windows(2).map(|w| distance(&w[0], &w[1])).collect();
    let acceleration: Vec<Option<f64>> = samples
        .windows(2)
        .enumerate()
        .map(|(i, w)| valid[i].then(|| (w[1].speed - w[0].speed) / 3.6 / intervals[i] / 9.80665))
        .collect();
    let mut output = String::new();
    let max_line =
        |output: &mut String, label: &str, values: &[Option<f64>], unit: &str, precision: usize| {
            let Some(max) = values.iter().flatten().copied().max_by(f64::total_cmp) else {
                writeln!(output, "{label:<18}N/A").unwrap();
                return;
            };
            let mut seconds = 0.0;
            let mut meters = 0.0;
            for (i, value) in values.iter().enumerate().take(intervals.len()) {
                if valid[i] && value.is_some_and(|v| (v - max).abs() < 1e-9) {
                    seconds += intervals[i];
                    meters += segments[i];
                }
            }
            writeln!(
                output,
                "{label:<18}{max:.precision$} {unit} (for {seconds:.0}s or {meters:.0}m)"
            )
            .unwrap();
        };
    max_line(
        &mut output,
        "Max engine speed:",
        &samples.iter().map(|s| Some(s.rpm)).collect::<Vec<_>>(),
        "rpm",
        0,
    );
    max_line(
        &mut output,
        "Max wheel speed:",
        &samples.iter().map(|s| Some(s.speed)).collect::<Vec<_>>(),
        "km/h",
        0,
    );
    max_line(
        &mut output,
        "Max acceleration:",
        &acceleration
            .iter()
            .map(|v| v.map(|v| v.max(0.0)))
            .collect::<Vec<_>>(),
        "g",
        2,
    );
    max_line(
        &mut output,
        "Max brake:",
        &acceleration
            .iter()
            .map(|v| v.map(|v| (-v).max(0.0)))
            .collect::<Vec<_>>(),
        "g",
        2,
    );
    max_line(
        &mut output,
        "Max water temp:",
        &samples.iter().map(|s| Some(s.temp)).collect::<Vec<_>>(),
        "°C",
        0,
    );
    let idle: Vec<f64> = samples
        .iter()
        .filter(|s| s.speed == 0.0 && s.rpm > 0.0)
        .map(|s| s.rpm)
        .collect();
    let moving: Vec<f64> = samples
        .iter()
        .filter(|s| s.speed > min_speed)
        .map(|s| s.speed)
        .collect();
    let mean = |v: &[f64]| {
        if v.is_empty() {
            "N/A".to_owned()
        } else {
            format!("{:.0}", v.iter().sum::<f64>() / v.len() as f64)
        }
    };
    writeln!(output, "{:<18}{} rpm", "Avg idle speed:", mean(&idle)).unwrap();
    writeln!(output, "{:<18}{} km/h", "Avg speed:", mean(&moving)).unwrap();
    let moving_median = if moving.is_empty() {
        "N/A".to_owned()
    } else {
        format!("{:.0}", median(&moving))
    };
    writeln!(output, "{:<18}{} km/h", "Median speed:", moving_median).unwrap();
    let first = &samples[0];
    let last = samples.last().unwrap();
    let seconds = ((last.ms - first.ms) / 1000.0).round() as u64;
    writeln!(
        output,
        "{:<18}{}:{:02}:{:02}",
        "Total time:",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
    .unwrap();
    let total: f64 = segments
        .iter()
        .zip(&valid)
        .filter(|(_, ok)| **ok)
        .map(|(d, _)| d)
        .sum();
    let straight = distance(first, last);
    writeln!(
        output,
        "{:<18}{:.2} km ({:.2} km straight)",
        "Distance:",
        total / 1000.0,
        straight / 1000.0
    )
    .unwrap();
    let dlon = (last.lon - first.lon).to_radians();
    let (lat1, lat2) = (first.lat.to_radians(), last.lat.to_radians());
    let bearing = (dlon.sin() * lat2.cos())
        .atan2(lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos())
        .to_degrees()
        .rem_euclid(360.0);
    let compass = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    if straight < 0.01 {
        writeln!(output, "{:<18}N/A", "Course:").unwrap();
    } else {
        writeln!(
            output,
            "{:<18}{} {:.0}°",
            "Course:",
            compass[((bearing / 45.0).round() as usize) % 8],
            bearing
        )
        .unwrap();
    }
    writeln!(output, "{:<18}{}", "Starting point:", position(first)).unwrap();
    writeln!(output, "{:<18}{}", "Ending point:", position(last)).unwrap();
    output
}

fn gear_report<'a>(samples: impl IntoIterator<Item = &'a Sample>) -> String {
    use std::fmt::Write;
    let mut output =
        String::from("\nMax for each gear\n--- --- ---- ----\n\n  Gear    rpm    km/h\n");
    let mut gears: BTreeMap<u8, (f64, f64)> = BTreeMap::new();
    for s in samples {
        if let Ok(gear @ 1..=6) = s.gear.parse::<u8>() {
            let entry = gears.entry(gear).or_default();
            entry.0 = entry.0.max(s.rpm);
            entry.1 = entry.1.max(s.speed);
        }
    }
    for (gear, (rpm, speed)) in gears {
        writeln!(output, "{gear:>6} {rpm:>6.0} {speed:>7.0}").unwrap();
    }
    output
}

// Ranges include brief stops but exclude leading and trailing stationary samples.
fn trips(samples: &[Sample], min_speed: f64, stop_seconds: f64) -> Vec<std::ops::Range<usize>> {
    let intervals: Vec<f64> = samples
        .windows(2)
        .map(|w| (w[1].ms - w[0].ms) / 1000.0)
        .collect();
    let gap_seconds = if intervals.is_empty() {
        1.5
    } else {
        median(&intervals) * 1.5
    };
    let mut ranges = Vec::new();
    let mut start = None;
    let mut last_moving = 0;
    let mut stopped_since = None;
    for (i, sample) in samples.iter().enumerate() {
        if i > 0 && (sample.ms - samples[i - 1].ms) / 1000.0 > gap_seconds {
            if let Some(first) = start.take() {
                ranges.push(first..last_moving + 1);
            }
            stopped_since = None;
        }
        if let Some(stopped) = stopped_since
            && (sample.ms - stopped) / 1000.0 >= stop_seconds
        {
            if let Some(first) = start.take() {
                ranges.push(first..last_moving + 1);
            }
            stopped_since = None;
        }
        if sample.speed > min_speed {
            start.get_or_insert(i);
            last_moving = i;
            stopped_since = None;
        } else if start.is_some() {
            stopped_since.get_or_insert(sample.ms);
        }
    }
    if let Some(first) = start {
        ranges.push(first..last_moving + 1);
    }
    ranges
}

fn elapsed(ms: f64) -> String {
    let seconds = (ms / 1000.0).round() as u64;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

fn trip_reports(title: &str, samples: &[Sample], min_speed: f64, stop_seconds: f64) -> String {
    use std::fmt::Write;
    let ranges = trips(samples, min_speed, stop_seconds);
    let mut output = heading(title)
        + &format!(
            "Detected trips: {} (speed > {min_speed} km/h; stop >= {stop_seconds}s)\n",
            ranges.len()
        );
    if ranges.is_empty() {
        output.push_str("No movement detected.\n");
    }
    for (i, range) in ranges.iter().enumerate() {
        let part = &samples[range.clone()];
        writeln!(
            output,
            "\nTrip {} | Elapsed range: {} - {} (from recording start)\n",
            i + 1,
            elapsed(part[0].ms - samples[0].ms),
            elapsed(part.last().unwrap().ms - samples[0].ms)
        )
        .unwrap();
        output.push_str(&report_body(part, min_speed));
    }
    if !ranges.is_empty() {
        output.push_str(&gear_report(
            ranges
                .iter()
                .flat_map(|range| samples[range.clone()].iter()),
        ));
    }
    output
}

fn markdown_text(value: &str) -> String {
    let mut escaped = String::new();
    for c in value.chars() {
        if c.is_control() {
            escaped.push(' ');
            continue;
        }
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\\' | '|' | '`' | '*' | '_' | '[' | ']' | '#' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

fn markdown_report(title: &str, plain_report: &str) -> String {
    use std::fmt::Write;
    let mut output = format!("# {}\n\n", markdown_text(title));
    let body = plain_report
        .strip_prefix(&heading(title))
        .unwrap_or(plain_report);
    let mut metrics = false;
    let mut gears = false;
    for line in body.lines().filter(|line| !line.is_empty()) {
        if line == "Max for each gear" {
            output.push_str(
                "\n## Max for each gear\n\n| Gear | rpm | km/h |\n| ---: | ---: | ---: |\n",
            );
            gears = true;
        } else if gears {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() == 3 && fields[0].parse::<u8>().is_ok() {
                writeln!(output, "| {} | {} | {} |", fields[0], fields[1], fields[2]).unwrap();
            }
        } else if line.starts_with("Trip ") {
            writeln!(output, "\n## {}\n", markdown_text(line)).unwrap();
            metrics = false;
        } else if line.starts_with("Detected trips:") || line == "No movement detected." {
            writeln!(output, "{}\n", markdown_text(line)).unwrap();
        } else if let Some((label, value)) = line.split_once(':') {
            if !metrics {
                output.push_str("| Metric | Value |\n| --- | --- |\n");
                metrics = true;
            }
            writeln!(
                output,
                "| {} | {} |",
                markdown_text(label.trim()),
                markdown_text(value.trim())
            )
            .unwrap();
        }
    }
    output
}

fn report_path(
    input: &std::path::Path,
    directory: Option<&std::path::Path>,
    extension: &str,
) -> Result<std::path::PathBuf> {
    let name = input.file_name().ok_or("Input path has no file name")?;
    let output = match directory {
        Some(directory) => directory.join(name).with_extension(extension),
        None => input.with_extension(extension),
    };
    if output == input
        || (output.exists() && fs::canonicalize(&output)? == fs::canonicalize(input)?)
    {
        return Err("Output path must not overwrite the input file".into());
    }
    Ok(output)
}

const USAGE: &str = "Usage: rideology2gpx <ride.csv> [--trips] [--min-speed KM/H] [--stop-seconds SECONDS] [--offline] [--overlay] [--overlay-fps FPS] [--overlay-size WIDTHxHEIGHT] [--redline-rpm RPM] [--temp-warning C] [--output-dir DIRECTORY] [--date \"YYYY-MM-DD HH:MM:SS\"]";

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let mut path = None;
    let mut date_value = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut output_dir = None;
    let mut per_trip = false;
    let mut offline = false;
    let mut min_speed = 3.0;
    let mut stop_seconds = 120.0;
    let mut custom_stop = false;
    let mut video = false;
    let mut video_options = overlay::Options {
        fps: 30,
        width: 1920,
        height: 512,
        redline_rpm: 10_000.0,
        temp_warning_c: 97.0,
    };
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "{USAGE}\n\nPrint a text ride report and save .md, .txt, and per-trip GPX tracks and speed, RPM, and gear JPG charts beside the input CSV. Supports UTF-8 and Shift-JIS CSV exports.\n\n--trips                 Report each moving period separately.\n--min-speed KM/H        Movement threshold (default: 3; strictly greater).\n--stop-seconds SECONDS  Minimum stop separating trips (default: 120).\n--output-dir, -o DIR    Write all files in DIR; create it if needed.\n--date DATE[ TIME]      Recording start, YYYY-MM-DD[ HH:MM:SS]; default: today at 00:00:00 local. RFC3339 offsets accepted.\n--offline               Skip all location lookups (coordinates only).
--overlay               Write an MP4 instrument video with black background for each trip (requires ffmpeg/libx264).
--overlay-fps FPS       Overlay frame rate (1-120; default: 30). Implies --overlay.
--overlay-size WxH      Even video dimensions (default: 1920x512). Implies --overlay.
--redline-rpm RPM      RPM where the bar turns red (default: 10000). Implies --overlay.
--temp-warning C       Temperature warning threshold in °C (default: 97). Implies --overlay.\n\nOnline mode sends report endpoint coordinates to Nominatim and caches area names.\n\n--stop-seconds requires --trips. --min-speed also filters average and median speed. Recording gaps always split trips."
                );
                println!("\n{}", geocoding::ATTRIBUTION);
                return Ok(());
            }
            Some("--date") => {
                let value = args.next().ok_or("--date requires YYYY-MM-DD")?;
                date_value = value
                    .to_str()
                    .ok_or("--date requires a date or datetime")?
                    .to_owned();
            }
            Some("--output-dir" | "-o") => {
                let directory = args
                    .next()
                    .filter(|v| !v.is_empty() && !v.to_string_lossy().starts_with('-'))
                    .ok_or("--output-dir requires a directory path")?;
                output_dir = Some(std::path::PathBuf::from(directory));
            }
            Some("--trips") => per_trip = true,
            Some("--overlay") => video = true,
            Some("--overlay-fps") => {
                video_options.fps = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse().ok()))
                    .filter(|v: &u32| (1..=120).contains(v))
                    .ok_or("--overlay-fps requires an integer from 1 to 120")?;
                video = true;
            }
            Some("--overlay-size") => {
                let value = args.next().ok_or("--overlay-size requires WIDTHxHEIGHT")?;
                let value = value
                    .to_str()
                    .ok_or("--overlay-size requires WIDTHxHEIGHT")?;
                let (w, h) = value
                    .split_once('x')
                    .ok_or("--overlay-size requires WIDTHxHEIGHT")?;
                video_options.width = w.parse().map_err(|_| "Invalid overlay width")?;
                video_options.height = h.parse().map_err(|_| "Invalid overlay height")?;
                if video_options.width < 480
                    || video_options.height < 128
                    || video_options.width > 3840
                    || video_options.height > 2160
                    || !video_options.width.is_multiple_of(2)
                    || !video_options.height.is_multiple_of(2)
                {
                    return Err(
                        "--overlay-size requires even dimensions from 480x128 to 3840x2160".into(),
                    );
                }
                video = true;
            }
            Some("--redline-rpm") => {
                video_options.redline_rpm = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse().ok()))
                    .filter(|v: &f64| v.is_finite() && *v > 0.0 && *v <= 100_000.0)
                    .ok_or("--redline-rpm requires a positive RPM value up to 100000")?;
                video = true;
            }
            Some("--temp-warning") => {
                video_options.temp_warning_c = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse().ok()))
                    .filter(|v: &f64| v.is_finite() && *v >= 0.0 && *v <= 500.0)
                    .ok_or("--temp-warning requires a Celsius value from 0 to 500")?;
                video = true;
            }
            Some("--offline") => offline = true,
            Some(flag @ ("--min-speed" | "--stop-seconds")) => {
                let value: f64 = args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse().ok()))
                    .ok_or_else(|| format!("{flag} requires a numeric value"))?;
                if !value.is_finite() || value < 0.0 || (flag == "--stop-seconds" && value == 0.0) {
                    return Err(format!("Invalid value for {flag}").into());
                }
                if flag == "--min-speed" {
                    min_speed = value;
                } else {
                    stop_seconds = value;
                    custom_stop = true;
                }
            }
            Some(flag) if flag.starts_with('-') => {
                return Err(format!("Unknown option: {flag}\n{USAGE}").into());
            }
            _ => {
                if path.replace(arg).is_some() {
                    return Err(USAGE.into());
                }
            }
        }
    }
    if custom_stop && !per_trip {
        return Err("--stop-seconds requires --trips".into());
    }
    let start_time = gpx::start_time(&date_value)?;
    let chart_date = start_time.format("%Y-%m-%d").to_string();
    let path = std::path::PathBuf::from(path.ok_or(USAGE)?);
    let destination = report_path(&path, output_dir.as_deref(), "md")?;
    let text_destination = report_path(&path, output_dir.as_deref(), "txt")?;
    let csv = fs::read(&path).map_err(|error| {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            env::current_dir().unwrap_or_default().join(&path)
        };
        format!("Cannot read input CSV {}: {error}", absolute.display())
    })?;
    let (title, mut samples) = read_ride(&csv)?;
    if let Some(parent) = destination.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let chart_ranges = trips(&samples, min_speed, stop_seconds);
    if !offline {
        let mut ranges = chart_ranges.clone();
        if !per_trip {
            ranges.push(0..samples.len());
        }
        let endpoints: std::collections::BTreeSet<_> =
            ranges.iter().flat_map(|r| [r.start, r.end - 1]).collect();
        if !endpoints.is_empty() {
            match geocoding::Geocoder::new() {
                Ok(geocoder) => {
                    for index in endpoints {
                        match geocoder.lookup(samples[index].lat, samples[index].lon) {
                            Ok(area) => {
                                samples[index].area =
                                    Some(area.unwrap_or_else(|| "area unavailable".into()));
                            }
                            Err(error) => {
                                eprintln!(
                                    "Warning: location lookup failed; remaining endpoints use coordinates only: {error}"
                                );
                                break;
                            }
                        }
                    }
                }
                Err(error) => eprintln!(
                    "Warning: location lookup unavailable; using coordinates only: {error}"
                ),
            }
        }
    }
    let output = if per_trip {
        trip_reports(&title, &samples, min_speed, stop_seconds)
    } else {
        report(&title, &samples, min_speed)
    };
    let chart_directory = destination
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    for (i, range) in chart_ranges.iter().enumerate() {
        let chart_path = charts::chart_path(&path, chart_directory, i + 1)?;
        if chart_path.exists() && fs::canonicalize(&chart_path)? == fs::canonicalize(&path)? {
            return Err("Chart path must not overwrite the input file".into());
        }
        let gpx_path = chart_path.with_extension("gpx");
        if gpx_path.exists() && fs::canonicalize(&gpx_path)? == fs::canonicalize(&path)? {
            return Err("GPX path must not overwrite the input file".into());
        }
        let gpx = gpx::render(
            &title,
            i + 1,
            &samples[range.clone()],
            start_time,
            samples[0].ms,
        )?;
        fs::write(&gpx_path, gpx)?;
        eprintln!("GPX saved to {}", gpx_path.display());
        charts::write_trip_chart(
            &chart_path,
            &samples[range.clone()],
            &title,
            i + 1,
            &chart_date,
        )?;
        eprintln!("Chart saved to {}", chart_path.display());
        if video {
            let video_path = overlay::path(&path, chart_directory, i + 1)?;
            overlay::write(&video_path, &samples[range.clone()], &video_options)?;
            eprintln!("Overlay saved to {}", video_path.display());
        }
    }
    fs::write(&destination, markdown_report(&title, &output))?;
    fs::write(&text_destination, &output)?;
    {
        use std::io::Write;
        std::io::stdout().lock().write_all(output.as_bytes())?;
    }
    eprintln!(
        "Reports saved to {} and {}",
        destination.display(),
        text_destination.display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: f64, speed: f64) -> Sample {
        Sample {
            ms,
            speed,
            lat: 0.0,
            lon: 0.0,
            rpm: 1000.0,
            temp: 90.0,
            gear: "1".into(),
            area: None,
        }
    }

    #[test]
    fn chart_dates_require_valid_calendar_dates_and_exact_format() {
        assert_eq!(
            gpx::start_time("2024-02-29")
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2024-02-29"
        );
        for invalid in [
            "2025-02-29",
            "2026-13-01",
            "2026-04-31",
            "2026-9-2",
            "today",
            "",
        ] {
            assert!(gpx::start_time(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn movement_statistics_exclude_stops_and_respect_threshold() {
        let samples: Vec<_> = [0.0, 1.0, 3.0, 10.0, 20.0, 60.0]
            .iter()
            .enumerate()
            .map(|(i, speed)| sample(i as f64 * 1000.0, *speed))
            .collect();
        let text = report("Test", &samples, 3.0);
        assert!(text.contains("Avg speed:        30 km/h"));
        assert!(text.contains("Median speed:     20 km/h"));
        let text = report("Test", &samples, 20.0);
        assert!(text.contains("Avg speed:        60 km/h"));
        assert!(text.contains("Median speed:     60 km/h"));
        let text = report("Test", &samples, 60.0);
        assert!(text.contains("Avg speed:        N/A km/h"));
        assert!(text.contains("Median speed:     N/A km/h"));
    }

    #[test]
    fn trips_have_one_shared_heading() {
        let (title, samples) = read_ride(include_bytes!("../tigre.csv")).unwrap();
        let text = trip_reports(&title, &samples, 3.0, 120.0);
        assert!(text.starts_with(&heading(&title)));
        assert_eq!(text.matches(&title).count(), 1);
        assert!(!text.contains(" - Trip"));
        assert_eq!(text.matches("Elapsed range:").count(), 3);
    }

    #[test]
    fn short_stops_stay_in_trip_and_long_stops_split() {
        let speeds = [0.0, 10.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0];
        let samples: Vec<_> = speeds
            .iter()
            .enumerate()
            .map(|(i, speed)| sample(i as f64 * 1000.0, *speed))
            .collect();
        assert_eq!(trips(&samples, 3.0, 3.0), vec![1..4, 7..8]);
        assert_eq!(trips(&samples, 3.0, 4.0), vec![1..8]);
        assert_eq!(trips(&samples, 10.0, 3.0), vec![]);
        assert!(trip_reports("Test", &samples, 3.0, 3.0).contains("Trip 2"));
    }

    #[test]
    fn recording_gaps_split_even_when_endpoints_are_moving() {
        let samples = vec![
            sample(0.0, 10.0),
            sample(1000.0, 10.0),
            sample(100000.0, 10.0),
            sample(101000.0, 10.0),
        ];
        assert_eq!(trips(&samples, 3.0, 120.0), vec![0..2, 2..4]);
    }

    #[test]
    fn stationary_recording_and_single_moving_sample() {
        let samples = vec![sample(0.0, 0.0), sample(1000.0, 0.0)];
        assert!(trip_reports("Test", &samples, 3.0, 120.0).contains("No movement detected"));
        let samples = vec![sample(0.0, 0.0), sample(1000.0, 10.0), sample(2000.0, 0.0)];
        assert_eq!(trips(&samples, 3.0, 120.0), vec![1..2]);
        let text = trip_reports("Test", &samples, 3.0, 120.0);
        assert!(text.contains("Max acceleration: N/A"));
        assert!(text.contains("0:00:00"));
    }

    #[test]
    fn sample_export_has_three_main_trips() {
        let (_, samples) = read_ride(include_bytes!("../tigre.csv")).unwrap();
        let ranges = trips(&samples, 3.0, 120.0);
        assert_eq!(ranges.len(), 3);
        let boundaries: Vec<_> = ranges
            .iter()
            .map(|r| {
                (
                    elapsed(samples[r.start].ms - samples[0].ms),
                    elapsed(samples[r.end - 1].ms - samples[0].ms),
                )
            })
            .collect();
        assert_eq!(
            boundaries,
            vec![
                ("0:02:06".into(), "0:06:08".into()),
                ("0:27:42".into(), "0:42:38".into()),
                ("1:57:47".into(), "2:23:54".into())
            ]
        );
        assert_eq!(trips(&samples, 3.0, 60.0).len(), 4);
    }

    #[test]
    fn geodesy_and_coordinates() {
        let a = sample(0.0, 0.0);
        let mut b = a.clone();
        b.lon = 1.0;
        assert!((distance(&a, &b) - 111_194.927).abs() < 0.01);
        assert_eq!(coordinate(-58.513317, 'E', 'W'), "W058°30′47.94″");
        assert_eq!(coordinate(12.999999999, 'N', 'S'), "N013°00′00.00″");
        assert_eq!(median(&[1.0, 9.0, 3.0, 5.0]), 4.0);
    }

    #[test]
    fn gaps_do_not_create_acceleration_or_distance() {
        let mut samples = vec![
            sample(0.0, 0.0),
            sample(1000.0, 36.0),
            sample(2000.0, 36.0),
            sample(102000.0, 200.0),
            sample(103000.0, 200.0),
        ];
        samples[3].lon = 1.0;
        samples[4].lon = 1.0;
        let text = report("Test", &samples, 3.0);
        assert!(text.contains("Max acceleration: 1.02 g"));
        assert!(text.contains("Distance:         0.00 km"));
        assert!(text.contains("0:01:43"));
    }

    #[test]
    fn real_shift_jis_export() {
        let (title, samples) = read_ride(include_bytes!("../tigre.csv")).unwrap();
        assert_eq!(title, "Ida y vuelta a tigre");
        assert_eq!(samples.len(), 3324);
        let text = report(&title, &samples, 3.0);
        assert!(text.contains("10815 rpm"));
        assert!(text.contains("190 km/h"));
        assert!(text.contains("2:36:12"));
        assert!(text.contains("W058°30′47.94″"));
    }

    #[test]
    fn rejects_missing_and_invalid_data() {
        assert!(read_ride(b"Title,Empty\n").is_err());
        let original = fs::read("tigre.csv").unwrap();
        let (decoded, _, _) = encoding_rs::SHIFT_JIS.decode(&original);
        assert!(
            read_ride(
                decoded
                    .replace("91,-34.535473", "NaN,-34.535473")
                    .as_bytes()
            )
            .is_err()
        );
        assert!(
            read_ride(
                decoded
                    .replace("1091,-34.535473", "91,-34.535473")
                    .as_bytes()
            )
            .is_err()
        );
    }
}
