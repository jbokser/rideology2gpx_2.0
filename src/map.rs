use std::{
    ffi::OsString,
    fs,
    hash::{Hash, Hasher},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use plotters::prelude::*;
use reqwest::blocking::Client;

use crate::{Result, Sample, charts, gpx};

const WIDTH: u32 = 1024;
const HEIGHT: u32 = 768;
const TILE_SIZE: i32 = 256;
const TILE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
const MAX_TILE_DOWNLOADS: usize = 64;
static TILE_DOWNLOADS: AtomicUsize = AtomicUsize::new(0);

pub fn path(input: &Path, directory: &Path, trip: Option<usize>) -> Result<PathBuf> {
    let mut name = OsString::from(input.file_stem().ok_or("Input path has no file stem")?);
    name.push(trip.map_or_else(|| "-map.jpg".to_owned(), |n| format!("-trip-{n}-map.jpg")));
    Ok(directory.join(name))
}

fn mercator(lat: f64, lon: f64, zoom: u8) -> (f64, f64) {
    let scale = f64::from(TILE_SIZE) * 2_f64.powi(i32::from(zoom));
    let latitude = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let x = (lon + 180.0) / 360.0 * scale;
    let y = (1.0 - latitude.tan().asinh() / std::f64::consts::PI) / 2.0 * scale;
    (x, y)
}

fn map_points(positions: &[(f64, f64)]) -> (u8, (f64, f64), Vec<(i32, i32)>) {
    let first_lon = positions[0].1;
    let adjusted: Vec<_> = positions
        .iter()
        .map(|(lat, lon)| {
            let lon = first_lon + (lon - first_lon + 180.0).rem_euclid(360.0) - 180.0;
            (*lat, lon)
        })
        .collect();
    let (zoom, projected, bounds) = (0..=16)
        .rev()
        .find_map(|zoom| {
            let points: Vec<_> = adjusted
                .iter()
                .map(|(lat, lon)| mercator(*lat, *lon, zoom))
                .collect();
            let min_x = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let max_x = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let min_y = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let max_y = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            ((max_x - min_x <= f64::from(WIDTH - 160))
                && (max_y - min_y <= f64::from(HEIGHT - 180)))
            .then_some((zoom, points, (min_x, max_x, min_y, max_y)))
        })
        .unwrap();
    let origin = (
        (bounds.0 + bounds.1 - f64::from(WIDTH)) / 2.0,
        (bounds.2 + bounds.3 - f64::from(HEIGHT)) / 2.0,
    );
    let pixels = projected
        .iter()
        .map(|(x, y)| ((x - origin.0).round() as i32, (y - origin.1).round() as i32))
        .collect();
    (zoom, origin, pixels)
}

struct TileClient {
    client: Client,
    cache: PathBuf,
    endpoint: String,
}

impl TileClient {
    fn new() -> Result<Self> {
        let endpoint =
            std::env::var("RIDEOLOGY_MAP_TILE_URL").unwrap_or_else(|_| DEFAULT_TILE_URL.to_owned());
        if !endpoint.starts_with("https://")
            || !["{z}", "{x}", "{y}"]
                .iter()
                .all(|part| endpoint.contains(part))
        {
            return Err(
                "RIDEOLOGY_MAP_TILE_URL requires an HTTPS URL with {z}, {x}, and {y}".into(),
            );
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        endpoint.hash(&mut hasher);
        let cache = std::env::var_os("RIDEOLOGY_MAP_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".rideology-cache/tiles"))
            .join(format!("{:016x}", hasher.finish()));
        let client = Client::builder()
            .user_agent(concat!(
                "rideology2gpx/",
                env!("CARGO_PKG_VERSION"),
                " (user-requested ride map; contact: juan.bokser@gmail.com)"
            ))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            cache,
            endpoint,
        })
    }

    fn tile(&self, zoom: u8, x: i32, y: i32) -> Result<Vec<u8>> {
        fs::create_dir_all(&self.cache)?;
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.cache.join("requests.lock"))?;
        lock_file.lock()?;
        let path = self.cache.join(format!("{zoom}/{x}/{y}.png"));
        if fs::metadata(&path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < TILE_TTL)
        {
            return Ok(fs::read(path)?);
        }
        let timestamp_path = self.cache.join("last-request-ms");
        if let Ok(previous) = fs::read_to_string(&timestamp_path)
            && let Ok(previous) = previous.trim().parse::<u128>()
        {
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
            let remaining = 250_u128.saturating_sub(now.saturating_sub(previous));
            thread::sleep(Duration::from_millis(remaining as u64));
        }
        fs::write(
            timestamp_path,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .to_string(),
        )?;
        let url = self
            .endpoint
            .replace("{z}", &zoom.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string());
        if TILE_DOWNLOADS.fetch_add(1, Ordering::Relaxed) >= MAX_TILE_DOWNLOADS {
            return Err("Map tile download limit reached for this run".into());
        }
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|_| "Map tile request failed")?
            .error_for_status()
            .map_err(|error| {
                format!(
                    "Map tile server returned HTTP {}",
                    error.status().map_or(0, |status| status.as_u16())
                )
            })?;
        let bytes = response.bytes()?.to_vec();
        image::load_from_memory(&bytes)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("png.tmp");
        fs::write(&temporary, &bytes)?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::rename(temporary, path)?;
        Ok(bytes)
    }
}

fn draw_tiles(pixels: &mut [u8], zoom: u8, origin: (f64, f64)) -> Result<()> {
    let tiles = TileClient::new()?;
    let min_x = (origin.0 / f64::from(TILE_SIZE)).floor() as i32;
    let max_x = ((origin.0 + f64::from(WIDTH) - 1.0) / f64::from(TILE_SIZE)).floor() as i32;
    let min_y = (origin.1 / f64::from(TILE_SIZE)).floor() as i32;
    let max_y = ((origin.1 + f64::from(HEIGHT) - 1.0) / f64::from(TILE_SIZE)).floor() as i32;
    let world_tiles = 1_i32 << zoom;
    for ty in min_y..=max_y {
        if !(0..world_tiles).contains(&ty) {
            continue;
        }
        for tx in min_x..=max_x {
            let tile_x = tx.rem_euclid(world_tiles);
            let tile = image::load_from_memory(&tiles.tile(zoom, tile_x, ty)?)?.to_rgb8();
            if tile.dimensions() != (TILE_SIZE as u32, TILE_SIZE as u32) {
                return Err("Unexpected map tile dimensions".into());
            }
            let left = (f64::from(tx * TILE_SIZE) - origin.0).round() as i32;
            let top = (f64::from(ty * TILE_SIZE) - origin.1).round() as i32;
            for sy in 0..TILE_SIZE {
                let dy = top + sy;
                if !(0..HEIGHT as i32).contains(&dy) {
                    continue;
                }
                for sx in 0..TILE_SIZE {
                    let dx = left + sx;
                    if !(0..WIDTH as i32).contains(&dx) {
                        continue;
                    }
                    let dst = ((dy as u32 * WIDTH + dx as u32) * 3) as usize;
                    pixels[dst..dst + 3].copy_from_slice(&tile.get_pixel(sx as u32, sy as u32).0);
                }
            }
        }
    }
    Ok(())
}

pub fn write(
    path: &Path,
    samples: &[Sample],
    ride_title: &str,
    trip: Option<usize>,
    date: &str,
    offline: bool,
) -> Result<Option<String>> {
    if samples.is_empty() {
        return Err("Cannot draw an empty route map".into());
    }
    let positions = gpx::route_positions(samples);
    let (zoom, origin, route) = map_points(&positions);
    let mut pixels = vec![244_u8; (WIDTH * HEIGHT * 3) as usize];
    let tile_error = if offline {
        None
    } else {
        draw_tiles(&mut pixels, zoom, origin).err().map(|error| {
            pixels.fill(244);
            error.to_string()
        })
    };
    let plain_background = offline || tile_error.is_some();
    {
        let root = BitMapBackend::with_buffer(&mut pixels, (WIDTH, HEIGHT)).into_drawing_area();
        if plain_background {
            root.fill(&RGBColor(242, 246, 248))?;
            for x in (0..WIDTH as i32).step_by(100) {
                root.draw(&PathElement::new(
                    vec![(x, 0), (x, HEIGHT as i32)],
                    RGBColor(219, 228, 232).stroke_width(1),
                ))?;
            }
            for y in (0..HEIGHT as i32).step_by(100) {
                root.draw(&PathElement::new(
                    vec![(0, y), (WIDTH as i32, y)],
                    RGBColor(219, 228, 232).stroke_width(1),
                ))?;
            }
        }
        for range in gpx::recording_ranges(samples) {
            let points = route[range].to_vec();
            if points.len() > 1 {
                root.draw(&PathElement::new(points.clone(), WHITE.stroke_width(9)))?;
                root.draw(&PathElement::new(
                    points,
                    RGBColor(16, 91, 196).stroke_width(5),
                ))?;
            }
        }
        let max_speed = samples
            .iter()
            .map(|sample| sample.speed)
            .fold(0.0_f64, f64::max);
        let peak_index = samples
            .iter()
            .position(|sample| sample.speed == max_speed)
            .unwrap();
        let peak = route[peak_index];
        let max_label = format!("Max: {} km/h", charts::axis_label(max_speed));
        let label_style = ("sans-serif", 22).into_font().color(&BLACK);
        let (label_width, label_height) = root.estimate_text_size(&max_label, &label_style)?;
        let label_width = label_width as i32;
        let label_height = label_height as i32;
        let label_on_left = peak.0 > WIDTH as i32 / 2;
        let label_x = if label_on_left {
            (peak.0 - 16 - label_width).max(20)
        } else {
            (peak.0 + 16).min(WIDTH as i32 - 20 - label_width)
        };
        let label_y = (peak.1 + label_height / 2).clamp(55, HEIGHT as i32 - 45);
        root.draw(&Circle::new(peak, 5, RGBColor(180, 42, 38).filled()))?;
        let start = route[0];
        let end = route[route.len() - 1];
        root.draw(&Circle::new(start, 10, WHITE.filled()))?;
        root.draw(&Circle::new(start, 7, RGBColor(26, 150, 77).filled()))?;
        root.draw(&Circle::new(end, 10, WHITE.filled()))?;
        root.draw(&Circle::new(end, 7, RGBColor(210, 54, 54).filled()))?;
        let draw_label = |label: &str, position: (i32, i32), size: i32| -> Result<()> {
            for (dx, dy) in [
                (-2, 0),
                (2, 0),
                (0, -2),
                (0, 2),
                (-1, -1),
                (-1, 1),
                (1, -1),
                (1, 1),
            ] {
                root.draw(&Text::new(
                    label.to_owned(),
                    (position.0 + dx, position.1 + dy),
                    ("sans-serif", size).into_font().color(&WHITE),
                ))?;
            }
            root.draw(&Text::new(
                label.to_owned(),
                position,
                ("sans-serif", size).into_font().color(&BLACK),
            ))?;
            Ok(())
        };
        draw_label(&max_label, (label_x, label_y), 22)?;
        draw_label(
            &charts::chart_title(samples, ride_title, trip, date),
            (20, 36),
            25,
        )?;
        if !plain_background {
            draw_label(
                "Map data © OpenStreetMap contributors (ODbL) | https://www.openstreetmap.org/copyright",
                (15, HEIGHT as i32 - 12),
                18,
            )?;
        }
        root.present()?;
    }
    let file = fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    JpegEncoder::new_with_quality(&mut writer, 90).encode(
        &pixels,
        WIDTH,
        HEIGHT,
        ExtendedColorType::Rgb8,
    )?;
    Ok(tile_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_map_uses_local_route_and_writes_jpeg() {
        let samples = [
            Sample {
                ms: 0.0,
                lat: -34.5,
                lon: -58.4,
                rpm: 1000.0,
                speed: 10.0,
                temp: 90.0,
                gear: "1".into(),
                area: None,
            },
            Sample {
                ms: 1000.0,
                lat: -34.501,
                lon: -58.401,
                rpm: 2000.0,
                speed: 20.0,
                temp: 90.0,
                gear: "2".into(),
                area: None,
            },
        ];
        let path = std::env::temp_dir().join(format!("rideology-map-{}.jpg", std::process::id()));
        write(&path, &samples, "Test", None, "2026-09-22", true).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]));
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (WIDTH, HEIGHT));
    }
}
