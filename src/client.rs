//! HTTP client for the Monochrome / Hi-Fi Tidal proxy API
//!
//! All responses wrap a top-level `"version": "2.5"` field.
//!
//! Verified endpoint shapes (tested against https://api.monochrome.tf):
//!
//! GET /info/?id=<track_id>
//!   → { version, data: { id, title, duration, trackNumber, audioQuality,
//!                         artist: {id,name,picture}, album: {id,title,cover} } }
//!
//! GET /album/?id=<album_id>
//!   → { version, data: { id, title, duration, numberOfTracks, releaseDate, cover,
//!                         artist: {name}, items: [{ item: { track fields } }] } }
//!
//! GET /artist/?id=<artist_id>
//!   → { version, artist: { id, name, picture, popularity },
//!                cover: { id, name, "750": "https://..." } }
//!   (note: top-level key is "artist", NOT "data")
//!
//! GET /search/?s=<q>&limit=N   (track search)
//!   → { version, data: { totalNumberOfItems, items: [ track objects ] } }
//!
//! GET /search/?al=<q>&limit=N  (album search)
//!   → { version, data: { albums: { items: [ album objects ] }, artists: { items: [] } } }
//!
//! GET /search/?a=<q>&limit=N   (artist search)
//!   → { version, data: { artists: { items: [ artist objects ] } } }
//!
//! GET /lyrics/?id=<track_id>
//!   → { version, lyrics: { lyrics: "plain text", subtitles: "lrc string" } }
//!   subtitles format: "[MM:SS.cc] text\n[MM:SS.cc] text\n..."
//!
//! GET /recommendations/?id=<track_id>
//!   → { version, data: { items: [{ track: { track fields } }] } }
//!   (note: items wrap with "track" key)
//!
//! GET /cover/?id=<track_id>
//!   → { version, covers: [{ id, name, "1280": "url", "640": "url", "80": "url" }] }
//!   (note: size keys are strings: "1280", "640", "80")

use crate::types::{MonoEvent, SearchKind};
use base64::Engine as _;
use serde_json::Value;

/// HTTP client wrapping the Monochrome API.
pub struct MonoClient {
    client: reqwest::Client,
    base_url: String,
}

impl MonoClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("plexus-mono/0.1.0")
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("failed to build reqwest client"),
            base_url: base_url.into(),
        }
    }

    pub fn default_instance() -> Self {
        Self::new("https://api.monochrome.tf")
    }

    /// Get the base URL for diagnostic/sanity checks.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        tracing::debug!("GET {}", url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(format!("HTTP {status} from {url}"));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| format!("failed to parse JSON: {e}"))
    }

    // ── Stream manifest ──────────────────────────────────────────────────────

    /// Resolve the stream manifest for a track.
    ///
    /// Returns a `MonoEvent::StreamManifest` with the direct pre-signed CDN URL.
    /// The URL is short-lived (~60s) — use it immediately.
    ///
    /// `quality`: "LOSSLESS" (default), "HI_RES_LOSSLESS", "HIGH", "LOW"
    pub async fn stream_manifest(&self, id: u64, quality: &str) -> Result<MonoEvent, String> {
        let json = self.get(&format!("/track/?id={id}&quality={quality}")).await?;
        let data = &json["data"];

        // Manifest is a base64-encoded JSON blob
        let manifest_b64 = data["manifest"].as_str()
            .ok_or("missing manifest field")?;
        let manifest_bytes = base64::engine::general_purpose::STANDARD
            .decode(manifest_b64)
            .map_err(|e| format!("base64 decode failed: {e}"))?;
        let manifest: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| format!("manifest JSON parse failed: {e}"))?;

        let mime_type = s(&manifest["mimeType"]);
        let codecs = s(&manifest["codecs"]);
        let url = manifest["urls"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|u| u.as_str())
            .ok_or("no URLs in manifest")?
            .to_string();

        let extension = mime_to_ext(&mime_type);
        let bit_depth = data["bitDepth"].as_u64().map(|n| n as u32);
        let sample_rate = data["sampleRate"].as_u64().map(|n| n as u32);
        let actual_quality = s(&data["audioQuality"]);

        Ok(MonoEvent::StreamManifest {
            id,
            url,
            mime_type,
            codecs,
            quality: actual_quality,
            bit_depth,
            sample_rate,
            extension,
        })
    }

    /// Download audio to a file, streaming progress events via a channel.
    ///
    /// Returns a receiver that yields `DownloadProgress` events followed by `DownloadComplete`.
    /// `path` should be a file path (e.g. `/tmp/track.flac`).
    pub async fn download(
        &self,
        id: u64,
        quality: &str,
        path: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<MonoEvent>, String> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        // Resolve the manifest URL
        let manifest = self.stream_manifest(id, quality).await?;
        let (url, mime_type) = match &manifest {
            MonoEvent::StreamManifest { url, mime_type, .. } => {
                (url.clone(), mime_type.clone())
            }
            _ => return Err("unexpected event from stream_manifest".to_string()),
        };

        // GET the audio stream
        let resp = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("download request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("download HTTP {}", resp.status()));
        }

        let total_bytes = resp.content_length();
        let file = tokio::fs::File::create(path)
            .await
            .map_err(|e| format!("failed to create {path}: {e}"))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<MonoEvent>(16);
        let path = path.to_string();

        tokio::spawn(async move {
            let mut file = file;
            let mut stream = resp.bytes_stream();
            let mut bytes_downloaded: u64 = 0;

            let _ = tx.send(MonoEvent::DownloadProgress {
                path: path.clone(),
                bytes_downloaded: 0,
                total_bytes,
                percent: Some(0.0),
            }).await;

            const CHUNK_REPORT: u64 = 256 * 1024;
            let mut since_last_report: u64 = 0;

            while let Some(chunk) = stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(MonoEvent::Error { message: format!("stream error: {e}") }).await;
                        return;
                    }
                };
                if file.write_all(&chunk).await.is_err() {
                    return;
                }

                bytes_downloaded += chunk.len() as u64;
                since_last_report += chunk.len() as u64;

                if since_last_report >= CHUNK_REPORT {
                    since_last_report = 0;
                    let percent = total_bytes
                        .map(|t| (bytes_downloaded as f32 / t as f32) * 100.0);
                    let _ = tx.send(MonoEvent::DownloadProgress {
                        path: path.clone(),
                        bytes_downloaded,
                        total_bytes,
                        percent,
                    }).await;
                }
            }

            let _ = file.flush().await;

            let _ = tx.send(MonoEvent::DownloadComplete {
                path: path.clone(),
                bytes: bytes_downloaded,
                mime_type,
            }).await;
        });

        Ok(rx)
    }

    // ── Track ────────────────────────────────────────────────────────────────

    pub async fn track_info(&self, id: u64) -> Result<MonoEvent, String> {
        let json = self.get(&format!("/info/?id={id}")).await?;
        // Response: { version, data: { track fields } }
        let data = &json["data"];
        parse_track(data).ok_or_else(|| format!("could not parse track {id}"))
    }

    // ── Album ────────────────────────────────────────────────────────────────

    pub async fn album(&self, id: u64) -> Result<(MonoEvent, Vec<MonoEvent>), String> {
        let json = self.get(&format!("/album/?id={id}")).await?;
        // Response: { version, data: { album fields, items: [{item: track}] } }
        let data = &json["data"];

        let album_id = data["id"].as_u64().unwrap_or(id);
        let title = s(&data["title"]);
        let artist = s(&data["artist"]["name"]);
        let release_date = data["releaseDate"].as_str().map(str::to_string);
        let track_count = data["numberOfTracks"].as_u64().unwrap_or(0) as u32;
        let duration_secs = data["duration"].as_u64();
        let cover_id = data["cover"].as_str().map(str::to_string);

        let album = MonoEvent::Album {
            id: album_id,
            title,
            artist,
            release_date,
            track_count,
            duration_secs,
            cover_id,
        };

        let tracks: Vec<MonoEvent> = data["items"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .filter_map(|(i, entry)| {
                        // Each entry is { "item": { track fields } }
                        let track = &entry["item"];
                        let t = parse_track(track)?;
                        if let MonoEvent::Track {
                            id,
                            title,
                            artist,
                            duration_secs,
                            audio_quality,
                            ..
                        } = t
                        {
                            Some(MonoEvent::AlbumTrack {
                                position: (i + 1) as u32,
                                id,
                                title,
                                artist,
                                duration_secs,
                                audio_quality,
                            })
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok((album, tracks))
    }

    // ── Artist ───────────────────────────────────────────────────────────────

    pub async fn artist(&self, id: u64) -> Result<MonoEvent, String> {
        let json = self.get(&format!("/artist/?id={id}")).await?;
        // Response: { version, artist: { id, name, picture }, cover: { "750": url } }
        // NOTE: top-level key is "artist", not "data"
        let a = &json["artist"];
        let artist_id = a["id"].as_u64().unwrap_or(id);
        let name = s(&a["name"]);
        let picture_id = a["picture"].as_str().map(str::to_string);

        // Cover comes as a separate top-level object: { "750": "https://..." }
        // We capture the 750px URL as the cover
        let cover_url = json["cover"]["750"].as_str().map(str::to_string);

        Ok(MonoEvent::Artist {
            id: artist_id,
            name,
            picture_id,
            cover_url,
        })
    }

    // ── Search ───────────────────────────────────────────────────────────────

    pub async fn search(
        &self,
        query: &str,
        kind: &SearchKind,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<MonoEvent>, String> {
        let encoded = url_encode(query);
        let (param, _label) = match kind {
            SearchKind::Tracks => ("s", "tracks"),
            SearchKind::Albums => ("al", "albums"),
            SearchKind::Artists => ("a", "artists"),
        };
        let path = format!("/search/?{param}={encoded}&limit={limit}&offset={offset}");
        let json = self.get(&path).await?;
        let data = &json["data"];

        match kind {
            SearchKind::Tracks => {
                // data: { items: [ direct track objects ] }
                let items = data["items"].as_array().ok_or("missing data.items")?;
                Ok(items
                    .iter()
                    .enumerate()
                    .filter_map(|(rank, item)| {
                        let t = parse_track(item)?;
                        if let MonoEvent::Track {
                            id,
                            title,
                            artist,
                            album,
                            duration_secs,
                            audio_quality,
                            ..
                        } = t
                        {
                            Some(MonoEvent::SearchTrack {
                                rank: rank as u32,
                                id,
                                title,
                                artist,
                                album,
                                duration_secs,
                                audio_quality,
                            })
                        } else {
                            None
                        }
                    })
                    .collect())
            }
            SearchKind::Albums => {
                // data: { albums: { items: [ album objects ] }, artists: { items: [] } }
                let items = data["albums"]["items"]
                    .as_array()
                    .ok_or("missing data.albums.items")?;
                Ok(items
                    .iter()
                    .enumerate()
                    .map(|(rank, item)| {
                        let id = item["id"].as_u64().unwrap_or(0);
                        let title = s(&item["title"]);
                        let artist = s(&item["artists"][0]["name"]);
                        let track_count =
                            item["numberOfTracks"].as_u64().unwrap_or(0) as u32;
                        let release_date =
                            item["releaseDate"].as_str().map(str::to_string);
                        MonoEvent::SearchAlbum {
                            rank: rank as u32,
                            id,
                            title,
                            artist,
                            track_count,
                            release_date,
                        }
                    })
                    .collect())
            }
            SearchKind::Artists => {
                // data: { artists: { items: [ artist objects ] } }
                let items = data["artists"]["items"]
                    .as_array()
                    .ok_or("missing data.artists.items")?;
                Ok(items
                    .iter()
                    .enumerate()
                    .map(|(rank, item)| {
                        let id = item["id"].as_u64().unwrap_or(0);
                        let name = s(&item["name"]);
                        MonoEvent::SearchArtist {
                            rank: rank as u32,
                            id,
                            name,
                        }
                    })
                    .collect())
            }
        }
    }

    // ── Lyrics ───────────────────────────────────────────────────────────────

    pub async fn lyrics(&self, id: u64) -> Result<Vec<MonoEvent>, String> {
        let json = self.get(&format!("/lyrics/?id={id}")).await?;
        // Response: { version, lyrics: { lyrics: "plain", subtitles: "lrc" } }
        let lobj = &json["lyrics"];

        // Prefer synced subtitles (LRC format) over plain lyrics
        if let Some(lrc) = lobj["subtitles"].as_str() {
            Ok(parse_lrc(lrc))
        } else if let Some(plain) = lobj["lyrics"].as_str() {
            Ok(plain
                .lines()
                .map(|line| MonoEvent::LyricLine {
                    timestamp_ms: None,
                    text: line.to_string(),
                })
                .collect())
        } else {
            Err("no lyrics found in response".to_string())
        }
    }

    // ── Recommendations ──────────────────────────────────────────────────────

    pub async fn recommendations(&self, id: u64) -> Result<Vec<MonoEvent>, String> {
        let json = self.get(&format!("/recommendations/?id={id}")).await?;
        // Response: { version, data: { items: [{ "track": { track fields } }] } }
        let items = json["data"]["items"]
            .as_array()
            .ok_or("missing data.items")?;

        Ok(items
            .iter()
            .enumerate()
            .filter_map(|(rank, entry)| {
                // Each entry is { "track": { track fields } }
                let track = &entry["track"];
                let t = parse_track(track)?;
                if let MonoEvent::Track {
                    id,
                    title,
                    artist,
                    duration_secs,
                    ..
                } = t
                {
                    Some(MonoEvent::Recommendation {
                        rank: rank as u32,
                        id,
                        title,
                        artist,
                        duration_secs,
                    })
                } else {
                    None
                }
            })
            .collect())
    }

    // ── Cover ────────────────────────────────────────────────────────────────

    pub async fn cover(&self, id: u64, size: u32) -> Result<Vec<MonoEvent>, String> {
        let json = self.get(&format!("/cover/?id={id}")).await?;
        // Response: { version, covers: [{ "1280": url, "640": url, "80": url }] }
        let covers = json["covers"]
            .as_array()
            .ok_or("missing covers array")?;

        let first = covers.first().ok_or("empty covers array")?;

        let sizes: &[u32] = if size == 0 { &[80, 640, 1280] } else { std::slice::from_ref(&size) };

        let mut events = Vec::new();
        for &s in sizes {
            let key = s.to_string();
            if let Some(url) = first[&key].as_str() {
                events.push(MonoEvent::Cover {
                    url: url.to_string(),
                    size: s,
                });
            }
        }

        if events.is_empty() {
            Err(format!("no cover URL found for size {size} (track {id})"))
        } else {
            Ok(events)
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Extract string value from JSON, defaulting to empty string.
fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// Parse a track JSON object into a `MonoEvent::Track`.
/// Returns None if `id` is missing (skips garbage entries).
fn parse_track(v: &Value) -> Option<MonoEvent> {
    let id = v["id"].as_u64()?;
    let title = s(&v["title"]);
    let version = v["version"].as_str().unwrap_or("");
    let full_title = if version.is_empty() {
        title
    } else {
        format!("{title} ({version})")
    };

    let artist = s(&v["artist"]["name"]);
    let album = s(&v["album"]["title"]);
    let album_id = v["album"]["id"].as_u64().unwrap_or(0);
    let duration_secs = v["duration"].as_u64().unwrap_or(0);
    let track_number = v["trackNumber"].as_u64().map(|n| n as u32);
    let release_date = v["streamStartDate"]
        .as_str()
        .or_else(|| v["releaseDate"].as_str())
        .map(str::to_string);
    let audio_quality = v["audioQuality"].as_str().map(str::to_string);
    let cover_id = v["album"]["cover"].as_str().map(str::to_string);

    Some(MonoEvent::Track {
        id,
        title: full_title,
        artist,
        album,
        album_id,
        duration_secs,
        track_number,
        release_date,
        audio_quality,
        cover_id,
    })
}

/// Parse LRC (lyric) format into `MonoEvent::LyricLine` events.
///
/// LRC format: `[MM:SS.cc] lyric text`
/// Example: `[01:00.66] But I'm a creep, I'm a weirdo`
fn parse_lrc(lrc: &str) -> Vec<MonoEvent> {
    lrc.lines()
        .filter_map(|line| {
            // Strip the [MM:SS.cc] prefix
            let line = line.trim();
            if line.starts_with('[') {
                let close = line.find(']')?;
                let timestamp_str = &line[1..close];
                let text = line[close + 1..].trim().to_string();

                // Parse [MM:SS.cc]
                let ts_ms = parse_lrc_timestamp(timestamp_str);

                Some(MonoEvent::LyricLine {
                    timestamp_ms: ts_ms,
                    text,
                })
            } else if !line.is_empty() {
                Some(MonoEvent::LyricLine {
                    timestamp_ms: None,
                    text: line.to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

/// Parse "MM:SS.cc" → milliseconds. Returns None on failure.
fn parse_lrc_timestamp(s: &str) -> Option<u64> {
    let colon = s.find(':')?;
    let mm: u64 = s[..colon].parse().ok()?;
    let rest = &s[colon + 1..];
    let (ss_str, cc_str) = rest.split_once('.').unwrap_or((rest, "0"));
    let ss: u64 = ss_str.parse().ok()?;
    let cc: u64 = cc_str.parse().ok().unwrap_or(0);
    // cc is centiseconds (2 digits = 1/100 sec)
    Some(mm * 60_000 + ss * 1_000 + cc * 10)
}

/// Map MIME type to a file extension.
fn mime_to_ext(mime: &str) -> String {
    match mime {
        "audio/flac" => "flac",
        "audio/mp4" | "audio/m4a" => "m4a",
        "audio/mpeg" => "mp3",
        "audio/ogg" => "ogg",
        "audio/webm" => "webm",
        _ => "audio",
    }
    .to_string()
}

/// Simple URL percent-encoding.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
                out.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
            }
        }
    }
    out
}
