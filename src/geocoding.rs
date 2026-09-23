use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::blocking::Client;
use serde_json::Value;

use crate::Result;

pub const ATTRIBUTION: &str = "Location data: © OpenStreetMap contributors (ODbL), via Nominatim. https://www.openstreetmap.org/copyright";

pub struct Geocoder {
    client: Client,
    endpoint: String,
    directory: PathBuf,
}

impl Geocoder {
    pub fn new() -> Result<Self> {
        let endpoint = env::var("NOMINATIM_URL")
            .unwrap_or_else(|_| "https://nominatim.openstreetmap.org/reverse".into());
        let directory = env::var_os("RIDEOLOGY_GEOCODE_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".rideology-cache"));
        fs::create_dir_all(&directory)?;
        let client = Client::builder()
            .user_agent(concat!(
                "rideology2gpx/",
                env!("CARGO_PKG_VERSION"),
                " (CLI ride endpoint reports)"
            ))
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            endpoint,
            directory,
        })
    }

    pub fn lookup(&self, lat: f64, lon: f64) -> Result<Option<String>> {
        // Share the lock and request timestamp across processes using this cache.
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.directory.join("requests.lock"))?;
        lock.lock()?;
        let cache_path = self.directory.join("areas.json");
        let mut cache: BTreeMap<String, Option<String>> = match fs::read(&cache_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        let key = format!("{}|es|{lat:.6},{lon:.6}", self.endpoint);
        if let Some(area) = cache.get(&key) {
            // Older cache entries include a display suffix that is no longer used.
            return Ok(area
                .as_ref()
                .map(|name| name.strip_suffix(" [locality]").unwrap_or(name).to_owned()));
        }
        let timestamp_path = self.directory.join("last-request-ms");
        if let Ok(value) = fs::read_to_string(&timestamp_path) {
            let previous: u128 = value.trim().parse()?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
            let remaining = 1100_u128.saturating_sub(now.saturating_sub(previous));
            thread::sleep(Duration::from_millis(remaining as u64));
        }
        fs::write(
            &timestamp_path,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .to_string(),
        )?;
        let response = self
            .client
            .get(&self.endpoint)
            .query(&[
                ("lat", format!("{lat:.6}")),
                ("lon", format!("{lon:.6}")),
                ("format", "jsonv2".into()),
                ("addressdetails", "1".into()),
                ("zoom", "18".into()),
                ("accept-language", "es".into()),
            ])
            .send()?
            .error_for_status()?
            .json::<Value>()?;
        let area = extract_area(&response)?;
        cache.insert(key, area.clone());
        let temporary = self.directory.join("areas.json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&cache)?)?;
        fs::rename(temporary, cache_path)?;
        Ok(area)
    }
}

fn extract_area(response: &Value) -> Result<Option<String>> {
    if response.get("error").is_some() {
        return Err("Nominatim could not resolve the coordinates".into());
    }
    let address = response
        .get("address")
        .and_then(Value::as_object)
        .ok_or("Nominatim returned no address details")?;
    for field in ["neighbourhood", "quarter", "suburb", "city_district"] {
        if let Some(name) = address
            .get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Ok(Some(clean_label(name)));
        }
    }
    for field in ["city", "town", "village", "municipality"] {
        if let Some(name) = address
            .get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return Ok(Some(clean_label(name)));
        }
    }
    Ok(None)
}

fn clean_label(label: &str) -> String {
    label.chars().filter(|c| !c.is_control()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn caches_results_and_spaces_requests() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Instant;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/reverse", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut requests = Vec::new();
            while requests.len() < 2 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut buffer = [0; 4096];
                        let count = stream.read(&mut buffer).unwrap();
                        let request = String::from_utf8_lossy(&buffer[..count]);
                        assert!(request.contains("format=jsonv2"));
                        requests.push(Instant::now());
                        let body = r#"{"address":{"neighbourhood":"Test area"}}"#;
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            requests
        });
        let directory = env::temp_dir().join(format!(
            "rideology-geocoding-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let geocoder = Geocoder {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            endpoint,
            directory: directory.clone(),
        };
        assert_eq!(
            geocoder.lookup(1.0, 1.0).unwrap().as_deref(),
            Some("Test area")
        );
        assert_eq!(
            geocoder.lookup(1.0, 1.0).unwrap().as_deref(),
            Some("Test area")
        );
        assert_eq!(
            geocoder.lookup(2.0, 2.0).unwrap().as_deref(),
            Some("Test area")
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].duration_since(requests[0]) >= Duration::from_secs(1));
        // A new client reuses persisted results without contacting the stopped server.
        let reopened = Geocoder {
            client: geocoder.client.clone(),
            endpoint: geocoder.endpoint.clone(),
            directory: directory.clone(),
        };
        assert_eq!(
            reopened.lookup(1.0, 1.0).unwrap().as_deref(),
            Some("Test area")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn prefers_neighborhood_to_broader_areas() {
        let response = json!({"address": {"city": "City", "suburb": "Suburb", "neighbourhood": "Neighborhood"}});
        assert_eq!(
            extract_area(&response).unwrap().as_deref(),
            Some("Neighborhood")
        );
        assert_eq!(
            extract_area(&json!({"address": {"suburb": "Suburb"}}))
                .unwrap()
                .as_deref(),
            Some("Suburb")
        );
    }

    #[test]
    fn locality_fallback_uses_plain_name_and_missing_names_are_not_invented() {
        assert_eq!(
            extract_area(&json!({"address": {"town": "Town"}}))
                .unwrap()
                .as_deref(),
            Some("Town")
        );
        assert_eq!(
            extract_area(&json!({"address": {"country": "Country"}})).unwrap(),
            None
        );
        assert!(extract_area(&json!({"error": "Unable to geocode"})).is_err());
        assert!(extract_area(&json!({})).is_err());
        assert_eq!(clean_label("Name\n\u{1b}"), "Name");
    }
}
