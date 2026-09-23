use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rideology-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("rides with spaces")).unwrap();
        fs::write(
            path.join("rides with spaces/trip.export.csv"),
            include_bytes!("../tigre.csv"),
        )
        .unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rideology2gpx"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn writes_markdown_and_text_beside_input_and_matches_stdout() {
    let workspace = Workspace::new();
    let input = workspace.0.join("rides with spaces/trip.export.csv");
    let original = fs::read(&input).unwrap();
    let result = workspace.run(&["rides with spaces/trip.export.csv", "--trips", "--offline"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        result.stdout,
        fs::read(input.with_extension("txt")).unwrap()
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("Max for each gear"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("Reports saved"));
    let report = fs::read_to_string(input.with_extension("md")).unwrap();
    assert!(report.starts_with("# Ida y vuelta a tigre\n\n"));
    assert_eq!(report.matches("## Trip ").count(), 3);
    assert_eq!(report.matches("## Max for each gear").count(), 1);
    assert!(report.contains("| Median speed | 80 km/h |"));
    assert!(report.contains("| 6 | 8914 | 190 |"));
    for trip in 1..=3 {
        let jpg = input
            .parent()
            .unwrap()
            .join(format!("trip.export-trip-{trip}.jpg"));
        assert!(jpg.with_extension("gpx").exists());
        let bytes = fs::read(jpg).unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]));
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1400, 1100));
    }
    assert_eq!(fs::read(input).unwrap(), original);
}

#[test]
fn creates_output_directories_and_replaces_previous_reports() {
    let workspace = Workspace::new();
    let args = [
        "rides with spaces/trip.export.csv",
        "--offline",
        "--output-dir",
        "reports/nested folder",
    ];
    assert!(workspace.run(&args).status.success());
    let output = workspace.0.join("reports/nested folder/trip.export.md");
    fs::write(&output, "Old report").unwrap();
    fs::write(output.with_extension("txt"), "Old text report").unwrap();
    let result = workspace.run(&args);
    assert!(result.status.success());
    assert_eq!(
        fs::read(output.with_extension("txt")).unwrap(),
        result.stdout
    );
    assert!(
        fs::read_to_string(output)
            .unwrap()
            .contains("| Max engine speed |")
    );
    let absolute_dir = workspace.0.join("absolute destination");
    assert!(
        workspace
            .run(&[
                "rides with spaces/trip.export.csv",
                "--offline",
                "-o",
                absolute_dir.to_str().unwrap()
            ])
            .status
            .success()
    );
    assert!(absolute_dir.join("trip.export.md").exists());
    assert!(absolute_dir.join("trip.export.txt").exists());
    assert!(absolute_dir.join("trip.export-trip-3.jpg").exists());
    assert!(
        workspace
            .0
            .join("reports/nested folder/trip.export-trip-1.jpg")
            .exists()
    );
}

#[test]
fn rejects_invalid_destinations_and_preserves_input() {
    let workspace = Workspace::new();
    assert!(
        !workspace
            .run(&["rides with spaces/trip.export.csv", "--output-dir"])
            .status
            .success()
    );
    assert!(
        !workspace
            .run(&[
                "rides with spaces/trip.export.csv",
                "--output-dir",
                "--offline"
            ])
            .status
            .success()
    );
    fs::write(workspace.0.join("not-a-directory"), "keep").unwrap();
    assert!(
        !workspace
            .run(&[
                "rides with spaces/trip.export.csv",
                "--offline",
                "-o",
                "not-a-directory"
            ])
            .status
            .success()
    );
    let input = workspace.0.join("input.md");
    fs::write(&input, include_bytes!("../tigre.csv")).unwrap();
    assert!(!workspace.run(&["input.md", "--offline"]).status.success());
    assert_eq!(fs::read(input).unwrap(), include_bytes!("../tigre.csv"));
    let text_input = workspace.0.join("other.txt");
    fs::write(&text_input, include_bytes!("../tigre.csv")).unwrap();
    assert!(!workspace.run(&["other.txt", "--offline"]).status.success());
    assert_eq!(
        fs::read(text_input).unwrap(),
        include_bytes!("../tigre.csv")
    );
    assert!(!workspace.0.join("other.md").exists());
}

#[test]
fn single_point_trip_renders_and_stationary_recording_has_no_charts() {
    let workspace = Workspace::new();
    let csv = "Title,Short ride\nelapsed_msec,gps_latitude,gps_longitude,engine_RPM,wheel_speed(km/h),water_temperature(C),gear_position\n0,0,0,1000,0,90,N\n1000,0,0,1000,10,90,1\n2000,0,0,1000,0,90,N\n";
    fs::write(workspace.0.join("single.csv"), csv).unwrap();
    let result = workspace.run(&["single.csv", "--offline", "--trips", "--date", "2024-02-29"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(workspace.0.join("single-trip-1.jpg")).unwrap();
    assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 1400);
    fs::write(
        workspace.0.join("stationary.csv"),
        csv.replace(",10,90,1", ",0,90,1"),
    )
    .unwrap();
    let result = workspace.run(&[
        "stationary.csv",
        "--offline",
        "--trips",
        "-o",
        "stationary-output",
    ]);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("No movement detected"));
    assert_eq!(
        fs::read_dir(workspace.0.join("stationary-output"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn invalid_date_fails_before_writing_output() {
    let workspace = Workspace::new();
    for args in [
        vec![
            "rides with spaces/trip.export.csv",
            "--offline",
            "--date",
            "2025-02-29",
        ],
        vec!["rides with spaces/trip.export.csv", "--offline", "--date"],
    ] {
        let result = workspace.run(&args);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("--date"));
    }
    assert!(
        !workspace
            .0
            .join("rides with spaces/trip.export.md")
            .exists()
    );
}

#[test]
fn gpx_uses_recording_time_across_trips_and_freezes_stopped_points() {
    let workspace = Workspace::new();
    let csv = "Title,A & B\nelapsed_msec,gps_latitude,gps_longitude,engine_RPM,wheel_speed(km/h),water_temperature(C),gear_position\n91,0,0,1000,0,90,N\n1091,0,0.0001,8045,36,90,6\n2091,0,0.00012,1000,0,90,1\n3091,0,0.00009,1000,0,90,1\n4091,0,0.0002,8000,36,90,6\n100091,0,0.001,8000,36,90,6\n101091,0,0.0011,8000,36,90,6\n";
    fs::write(workspace.0.join("timed.csv"), csv).unwrap();
    let result = workspace.run(&[
        "timed.csv",
        "--offline",
        "--date",
        "2026-09-22T23:59:59-03:00",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let ns = "http://www.topografix.com/GPX/1/1";
    for (trip, first_time, count) in [
        (1, "2026-09-23T03:00:00.000Z", 4),
        (2, "2026-09-23T03:01:39.000Z", 2),
    ] {
        let content =
            fs::read_to_string(workspace.0.join(format!("timed-trip-{trip}.gpx"))).unwrap();
        let doc = roxmltree::Document::parse(&content).unwrap();
        assert!(doc.root_element().has_tag_name((ns, "gpx")));
        let points: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name((ns, "trkpt")))
            .collect();
        assert_eq!(points.len(), count);
        assert_eq!(
            points[0]
                .children()
                .find(|n| n.has_tag_name((ns, "time")))
                .unwrap()
                .text(),
            Some(first_time)
        );
        let waypoints: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name((ns, "wpt")))
            .collect();
        assert_eq!(waypoints.len(), 3);
        assert_eq!(waypoints[0].attribute("lat"), points[0].attribute("lat"));
        assert_eq!(
            waypoints[1].attribute("lon"),
            points.last().unwrap().attribute("lon")
        );
        assert_eq!(
            waypoints[2]
                .children()
                .find(|n| n.has_tag_name((ns, "name")))
                .unwrap()
                .text(),
            Some("Max speed 36 km/h")
        );
        if trip == 1 {
            for i in 1..=2 {
                assert_eq!(points[0].attribute("lat"), points[i].attribute("lat"));
                assert_eq!(points[0].attribute("lon"), points[i].attribute("lon"));
            }
            assert!(content.contains("8045rpm @ 6 gear"));
        }
    }
}
