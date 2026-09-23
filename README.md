# **$ rideology2gpx**

A Rust CLI that generate `.gpx` and some other data report from a Kawasaki Rideology CSV export.

```bash
cargo run --release -- file.csv
# Save to a different directory (created if needed):
cargo run --release -- file.csv --output-dir reports
# Run the compiled binary:
./target/release/rideology2gpx tigre.csv
```

### GPS jitter correction

GPX coordinates are reconstructed; report statistics and JPGs continue using the original GPS data. The algorithm fits GPS observations and wheel-distance constraints together in a local metric projection. For the interval ending at sample `i`, the target distance is `wheel_speed[i] / 3.6 × actual_elapsed_seconds`. Consecutive samples with zero wheel speed share exactly one position, including their arrival interval, so stopped points do not wander. Their GPS anchor is the median position of the stationary group.

For moving points, 200 bidirectional constraint-fitting passes adjust neighboring positions toward the wheel distance, with a small attraction (2.5% per pass) toward GPS anchors to limit drift. This is a compromise: derived GPS speed approximates wheel speed; it is not guaranteed to match it exactly. Coordinates and start/end locations can move slightly. No map matching is performed, turns can be softened, and the result is not a surveyed or exact original trajectory. If GPS fixes coincide while the wheel indicates motion, a nearby GPS heading is used when available; without any heading information, the exporter does not invent a direction. The local projection is intended for regional rides, not polar or globe-spanning tracks.

Trip boundaries prevent interpolation across missing telemetry. The fitted trajectory does not remove samples or change their times. A viewer that calculates speed from consecutive points should show less jitter, although its own filtering and interpolation may affect the displayed result.

### Trip charts

Every run also generates one JPEG chart with speed, RPM, and gear per detected trip

Charts are saved beside the reports and honor `--output-dir` / `-o`. The input stem is preserved (for example, `ride.export.csv` produces `ride.export-trip-1.jpg`). Trip numbering starts at 1. Files with matching names are replaced on subsequent runs; older charts with other trip numbers are not automatically deleted if detection settings change.

Rendering uses Plotters and system fonts. On Debian/Ubuntu, building requires `pkg-config`, `libfreetype6-dev`, and `libfontconfig1-dev`; install a sans-serif font such as `fonts-dejavu-core` for rendering.


A trip starts when wheel speed exceeds `--min-speed` (default: 3 km/h). Samples at or below that threshold count as stopped. A continuous observed stop lasting at least `--stop-seconds` (default: 120) splits trips. Shorter stops remain inside a trip, so ordinary traffic stops need not create new reports. Stop duration is measured from the first stopped sample to the current sample, including a resuming sample when checking the threshold.

Recording gaps longer than 1.5 times the recording's median sampling interval always split trips. A gap means missing data, not confirmed stationary time. Leading and trailing stationary samples are excluded: each trip runs from its first moving sample to its last, including any short stops between them. No acceleration or distance is calculated across trip boundaries. A single moving sample is retained as a zero-duration trip, with unavailable acceleration and braking shown as N/A. An entirely stationary recording produces `No movement detected.`

The movement threshold controls both segmentation and the samples used for average and median speed. Total trip time still includes brief stops. There is no minimum trip duration or additional noise filter.

### Endpoint neighborhoods

By default, the CLI sends only the reported start and end coordinates to [Nominatim](https://nominatim.org/), using OpenStreetMap data, and adds an area name beside each coordinate. In `--trips` mode it looks up the endpoints of each trip. No API key is required. Names are requested in Spanish and preserved as returned by the service.


**Public-service limits:** follow the [Nominatim usage policy](https://operations.osmfoundation.org/policies/nominatim/). This integration is for occasional, user-triggered reports, not scheduled or bulk processing. It sends requests sequentially, at least 1.1 seconds apart, identifies the application with a User-Agent, caches results persistently, and credits OpenStreetMap in this README and in `--help`. Processes sharing the same cache also share a request lock and rate limit. Do not run instances with different caches or on multiple machines concurrently against the public service; application-wide traffic must remain below one request per second. Use `--offline` for recordings whose endpoint coordinates should not be sent to the service.

Cache files are stored in `.rideology-cache/` under the current working directory (ignored by Git). They contain queried coordinates and area names, not whole ride traces. Set `RIDEOLOGY_GEOCODE_CACHE` to share a cache across working directories. Cached results do not expire automatically; delete `areas.json` to refresh names. `--offline` skips both cache reads and network lookups. A custom or self-hosted reverse-geocoding endpoint can be selected with `NOMINATIM_URL` without changing the code.

Location data: © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), available under the ODbL.

## Verification

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

Errors are printed to stderr and return exit code 1.
