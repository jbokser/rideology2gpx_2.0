use std::fmt::Write;

use crate::{EARTH_M, Result, Sample, median};
use chrono::{
    DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Timelike, Utc,
};

pub fn start_time(value: &str) -> Result<DateTime<FixedOffset>> {
    let invalid = "--date requires YYYY-MM-DD or YYYY-MM-DD HH:MM:SS (optional RFC3339 timezone)";
    if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        if date.nanosecond() >= 1_000_000_000 {
            return Err(invalid.into());
        }
        return Ok(date);
    }
    let naive = if value.len() == 10 {
        let day = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| invalid)?;
        if day.format("%Y-%m-%d").to_string() != value {
            return Err(invalid.into());
        }
        day.and_hms_opt(0, 0, 0).unwrap()
    } else {
        let normalized = value.replace('T', " ");
        let date =
            NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S").map_err(|_| invalid)?;
        if date.format("%Y-%m-%d %H:%M:%S").to_string() != normalized
            || date.nanosecond() >= 1_000_000_000
        {
            return Err(invalid.into());
        }
        date
    };
    Local.from_local_datetime(&naive).single().map(|date| date.fixed_offset())
        .ok_or_else(|| "--date is ambiguous or nonexistent in the local timezone; provide an explicit UTC offset".into())
}

fn xml(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            matches!(c, '\t' | '\n' | '\r') || (*c >= ' ' && *c != '\u{fffe}' && *c != '\u{ffff}')
        })
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// Fit positions to both GPS observations and wheel-distance constraints. Zero-speed
// intervals share a node, so no optimization step can introduce stationary jitter.
fn smooth_positions(samples: &[Sample]) -> Vec<(f64, f64)> {
    let origin = &samples[0];
    let cos_lat = origin.lat.to_radians().cos().abs().max(1e-6);
    let raw: Vec<_> = samples
        .iter()
        .map(|s| {
            (
                ((s.lon - origin.lon + 180.0).rem_euclid(360.0) - 180.0).to_radians()
                    * EARTH_M
                    * cos_lat,
                (s.lat - origin.lat).to_radians() * EARTH_M,
            )
        })
        .collect();
    let mut groups: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut steps = Vec::new();
    let mut ids = Vec::new();
    for (i, point) in raw.iter().enumerate() {
        let meters = if i == 0 {
            0.0
        } else {
            samples[i].speed / 3.6 * (samples[i].ms - samples[i - 1].ms) / 1000.0
        };
        if i == 0 || meters > 0.0 {
            groups.push(Vec::new());
            steps.push(meters);
        }
        groups.last_mut().unwrap().push(*point);
        ids.push(groups.len() - 1);
    }
    let targets: Vec<_> = groups
        .iter()
        .map(|group| {
            (
                median(&group.iter().map(|p| p.0).collect::<Vec<_>>()),
                median(&group.iter().map(|p| p.1).collect::<Vec<_>>()),
            )
        })
        .collect();
    let mut positions = targets.clone();
    for _ in 0..200 {
        for (p, target) in positions.iter_mut().zip(&targets) {
            p.0 += 0.025 * (target.0 - p.0);
            p.1 += 0.025 * (target.1 - p.1);
        }
        for reverse in [false, true] {
            for offset in 1..positions.len() {
                let i = if reverse {
                    positions.len() - offset
                } else {
                    offset
                };
                let mut dx = positions[i].0 - positions[i - 1].0;
                let mut dy = positions[i].1 - positions[i - 1].1;
                let mut norm = dx.hypot(dy);
                if norm < 1e-9 {
                    // Recover a local heading where consecutive GPS fixes coincide.
                    let before = i.saturating_sub(2);
                    let after = (i + 1).min(targets.len() - 1);
                    dx = targets[after].0 - targets[before].0;
                    dy = targets[after].1 - targets[before].1;
                    norm = dx.hypot(dy);
                    if norm < 1e-9 {
                        continue;
                    }
                    dx /= norm;
                    dy /= norm;
                    norm = 0.0;
                } else {
                    dx /= norm;
                    dy /= norm;
                }
                let correction = (norm - steps[i]) * 0.5;
                positions[i].0 -= dx * correction;
                positions[i].1 -= dy * correction;
                positions[i - 1].0 += dx * correction;
                positions[i - 1].1 += dy * correction;
            }
        }
    }
    ids.into_iter()
        .map(|i| {
            (
                origin.lat + (positions[i].1 / EARTH_M).to_degrees(),
                (origin.lon + (positions[i].0 / (EARTH_M * cos_lat)).to_degrees() + 180.0)
                    .rem_euclid(360.0)
                    - 180.0,
            )
        })
        .collect()
}

pub(crate) fn recording_ranges(samples: &[Sample]) -> Vec<std::ops::Range<usize>> {
    if samples.len() < 2 {
        return std::iter::once(0..samples.len()).collect();
    }
    let intervals: Vec<_> = samples
        .windows(2)
        .map(|pair| (pair[1].ms - pair[0].ms) / 1000.0)
        .collect();
    let gap_seconds = median(&intervals) * 1.5;
    let mut ranges = Vec::new();
    let mut start = 0;
    for (index, interval) in intervals.iter().enumerate() {
        if *interval > gap_seconds {
            ranges.push(start..index + 1);
            start = index + 1;
        }
    }
    ranges.push(start..samples.len());
    ranges
}

pub(crate) fn route_positions(samples: &[Sample]) -> Vec<(f64, f64)> {
    recording_ranges(samples)
        .iter()
        .flat_map(|range| smooth_positions(&samples[range.clone()]))
        .collect()
}

pub fn render(
    title: &str,
    trip: usize,
    samples: &[Sample],
    start: DateTime<FixedOffset>,
    recording_ms: f64,
) -> Result<String> {
    if samples.is_empty() {
        return Err("Cannot export an empty trip".into());
    }
    let ranges = recording_ranges(samples);
    let positions = route_positions(samples);
    if positions
        .iter()
        .any(|(lat, lon)| !lat.is_finite() || lat.abs() > 90.0 || !lon.is_finite())
    {
        return Err("Smoothed GPS positions are outside geographic bounds".into());
    }
    let times: Vec<_> = samples
        .iter()
        .map(|s| {
            let millis = (s.ms - recording_ms).round();
            if !millis.is_finite() || !(0.0..i64::MAX as f64).contains(&millis) {
                return Err("Elapsed time cannot be represented in GPX".into());
            }
            let delta = chrono::TimeDelta::try_milliseconds(millis as i64)
                .ok_or("Elapsed time is too large")?;
            start
                .with_timezone(&Utc)
                .checked_add_signed(delta)
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
                .ok_or_else(|| "GPX timestamp is out of range".into())
        })
        .collect::<Result<Vec<String>>>()?;
    let mut output = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"rideology2gpx\" xmlns=\"http://www.topografix.com/GPX/1/1\" xmlns:ride=\"urn:rideology2gpx:telemetry:1\">\n",
    );
    let name = xml(&format!("{title} - Trip {trip}"));
    writeln!(output, " <metadata><name>{name}</name><desc>GPS positions fitted to wheel speed; approximate reconstructed trajectory.</desc><time>{}</time></metadata>", times[0]).unwrap();
    let peak = samples.iter().enumerate().fold(0, |best, (i, s)| {
        if s.speed > samples[best].speed {
            i
        } else {
            best
        }
    });
    for (index, name, desc) in [
        (0, "start".to_owned(), String::new()),
        (samples.len() - 1, "end".to_owned(), String::new()),
        (
            peak,
            format!("Max speed {} km/h", samples[peak].speed),
            format!("{}rpm @ {} gear", samples[peak].rpm, samples[peak].gear),
        ),
    ] {
        let (lat, lon) = positions[index];
        writeln!(output, " <wpt lat=\"{lat:.9}\" lon=\"{lon:.9}\"><time>{}</time><name>{}</name><desc>{}</desc></wpt>", times[index], xml(&name), xml(&desc)).unwrap();
    }
    writeln!(output, " <trk><name>{name}</name>").unwrap();
    for range in ranges {
        output.push_str("  <trkseg>\n");
        for i in range {
            let s = &samples[i];
            let (lat, lon) = positions[i];
            writeln!(output, "   <trkpt lat=\"{lat:.9}\" lon=\"{lon:.9}\"><time>{}</time><extensions><ride:wheel_speed_kmh>{}</ride:wheel_speed_kmh><ride:engine_rpm>{}</ride:engine_rpm><ride:gear>{}</ride:gear></extensions></trkpt>", times[i], s.speed, s.rpm, xml(&s.gear)).unwrap();
        }
        output.push_str("  </trkseg>\n");
    }
    output.push_str(" </trk>\n</gpx>\n");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(ms: f64, x: f64, speed: f64) -> Sample {
        Sample {
            ms,
            lat: 0.0,
            lon: (x / EARTH_M).to_degrees(),
            speed,
            rpm: 8045.0,
            temp: 90.0,
            gear: "6".into(),
            area: None,
        }
    }
    #[test]
    fn stops_are_fixed_and_wheel_speed_reduces_gps_jitter() {
        let samples = [
            sample(0.0, 0.0, 36.0),
            sample(1000.0, 13.0, 36.0),
            sample(2000.0, 18.0, 36.0),
            sample(3000.0, 30.0, 36.0),
            sample(4000.0, 33.0, 0.0),
            sample(5000.0, 27.0, 0.0),
        ];
        let positions = smooth_positions(&samples);
        assert_eq!(positions[3], positions[4]);
        assert_eq!(positions[4], positions[5]);
        for i in 1..4 {
            let meters = (positions[i].1 - positions[i - 1].1).to_radians() * EARTH_M;
            assert!((meters - 10.0).abs() < 0.5, "{meters}");
        }
    }
    #[test]
    fn timestamps_keep_recording_offset_milliseconds_and_rollover() {
        let start = start_time("2026-09-22T23:59:59-03:00").unwrap();
        let samples = [sample(6091.0, 0.0, 36.0), sample(7092.0, 10.0, 36.0)];
        let text = render("A & <B>", 2, &samples, start, 91.0).unwrap();
        assert!(text.contains("2026-09-23T03:00:05.000Z"));
        assert!(text.contains("2026-09-23T03:00:06.001Z"));
        assert!(text.contains("A &amp; &lt;B&gt;"));
        assert!(text.contains("<name>start</name>"));
        assert!(text.contains("<name>end</name>"));
        assert!(text.contains("<desc>8045rpm @ 6 gear</desc>"));
        assert_eq!(text.matches("<wpt ").count(), 3);
        assert_eq!(text.matches("<trkpt ").count(), 2);
    }
    #[test]
    fn recording_gap_creates_segments_inside_one_gpx_track() {
        let samples = [
            sample(0.0, 0.0, 36.0),
            sample(1000.0, 10.0, 36.0),
            sample(100000.0, 100.0, 36.0),
            sample(101000.0, 110.0, 36.0),
        ];
        let gpx = render("Test", 1, &samples, start_time("2026-09-22").unwrap(), 0.0).unwrap();
        assert_eq!(gpx.matches("<trk>").count(), 1);
        assert_eq!(gpx.matches("<trkseg>").count(), 2);
        assert_eq!(gpx.matches("<trkpt ").count(), 4);
    }

    #[test]
    fn local_date_defaults_to_midnight_and_explicit_time_is_preserved() {
        let date = start_time("2026-09-22").unwrap();
        assert_eq!(date.format("%H:%M:%S").to_string(), "00:00:00");
        assert_eq!(
            start_time("2026-09-22 09:30:15")
                .unwrap()
                .format("%H:%M:%S")
                .to_string(),
            "09:30:15"
        );
        for bad in [
            "2026-02-30",
            "2026-09-22 25:00:00",
            "2026-09-22 12:00:60",
            "2026-9-2",
            "12:00:00",
        ] {
            assert!(start_time(bad).is_err(), "{bad}");
        }
    }
}
