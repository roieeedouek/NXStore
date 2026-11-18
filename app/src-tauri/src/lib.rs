//! Switch Library backend.
//!
//! Downloads the public titledb metadata catalog for a region, slims it down
//! to the fields the UI needs, and caches the result in the app data folder.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

const CATALOG_BASE: &str = "https://raw.githubusercontent.com/blawar/titledb/master";
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const REGIONS: &[&str] = &["US.en", "GB.en", "AU.en", "JP.ja", "DE.de", "FR.fr", "ES.es", "IT.it"];

/// One entry as it appears in titledb. Every field is optional there.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTitle {
    id: Option<String>,
    name: Option<String>,
    publisher: Option<String>,
    developer: Option<String>,
    release_date: Option<u32>,
    icon_url: Option<String>,
    banner_url: Option<String>,
    screenshots: Option<Vec<String>>,
    description: Option<String>,
    intro: Option<String>,
    category: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    number_of_players: Option<u32>,
    size: Option<u64>,
    is_demo: Option<bool>,
    rating_content: Option<Vec<String>>,
}

/// The slimmed-down record sent to the UI.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    id: String,
    name: String,
    publisher: Option<String>,
    developer: Option<String>,
    release_date: Option<u32>,
    icon_url: Option<String>,
    banner_url: Option<String>,
    screenshots: Vec<String>,
    description: Option<String>,
    intro: Option<String>,
    category: Vec<String>,
    languages: Vec<String>,
    players: Option<u32>,
    size: Option<u64>,
    demo: bool,
    rating_content: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    region: String,
    fetched_at: u64,
    from_cache: bool,
    games: Vec<Game>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    region: String,
    fetched_at: u64,
    games: Vec<Game>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    phase: &'static str,
    received: u64,
    total: Option<u64>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path(app: &AppHandle, region: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    Ok(dir.join(format!("catalog-{region}.json")))
}

/// Base titles only: application IDs end in `000`. Updates (`800`) and DLC are skipped.
fn is_base_title(id: &str) -> bool {
    id.len() == 16 && id.ends_with("000") && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// titledb leaves `isDemo` unset on many demos, so also check the name.
fn looks_like_demo(name: &str) -> bool {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == "demo")
}

fn slim(raw: HashMap<String, RawTitle>) -> Vec<Game> {
    let mut by_id: HashMap<String, Game> = HashMap::new();
    for t in raw.into_values() {
        let (Some(id), Some(name)) = (t.id, t.name) else { continue };
        let id = id.to_uppercase();
        let name = name.trim().to_string();
        if name.is_empty() || !is_base_title(&id) {
            continue;
        }
        let game = Game {
            id: id.clone(),
            demo: t.is_demo.unwrap_or(false) || looks_like_demo(&name),
            name,
            publisher: t.publisher.filter(|s| !s.is_empty()),
            developer: t.developer.filter(|s| !s.is_empty()),
            release_date: t.release_date.filter(|d| *d > 19_000_000),
            icon_url: t.icon_url,
            banner_url: t.banner_url,
            screenshots: t.screenshots.unwrap_or_default(),
            description: t.description.map(|d| d.trim().to_string()),
            intro: t.intro,
            category: t.category.unwrap_or_default(),
            languages: {
                let mut l = t.languages.unwrap_or_default();
                l.dedup();
                l
            },
            players: t.number_of_players,
            size: t.size.filter(|s| *s > 0),
            rating_content: t.rating_content.unwrap_or_default(),
        };
        // Some titles appear under several eShop IDs; keep the richest record.
        let richer = |g: &Game| g.screenshots.len() + g.description.as_ref().map_or(0, |_| 10);
        match by_id.get(&id) {
            Some(existing) if richer(existing) >= richer(&game) => {}
            _ => {
                by_id.insert(id, game);
            }
        }
    }
    by_id.into_values().collect()
}

async fn read_cache(app: &AppHandle, region: &str) -> Option<CacheFile> {
    let bytes = tokio::fs::read(cache_path(app, region).ok()?).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write_cache(app: &AppHandle, cache: &CacheFile) -> Result<(), String> {
    let path = cache_path(app, &cache.region)?;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec(cache).map_err(|e| e.to_string())?;
    tokio::fs::write(&tmp, body).await.map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())
}

async fn download(app: &AppHandle, region: &str) -> Result<Vec<Game>, String> {
    let url = format!("{CATALOG_BASE}/{region}.json");
    let client = reqwest::Client::builder()
        .user_agent(concat!("switch-library/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Could not download the catalog: {e}"))?;

    let total = resp.content_length();
    let mut body = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut stream = resp.bytes_stream();
    let mut last_emit = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        body.extend_from_slice(&chunk);
        let received = body.len() as u64;
        if received - last_emit > 512 * 1024 {
            last_emit = received;
            let _ = app.emit("catalog-progress", Progress { phase: "download", received, total });
        }
    }
    let _ = app.emit(
        "catalog-progress",
        Progress { phase: "parse", received: body.len() as u64, total },
    );

    let raw: HashMap<String, RawTitle> = tauri::async_runtime::spawn_blocking(move || {
        serde_json::from_slice(&body).map_err(|e| format!("Catalog format not recognised: {e}"))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(slim(raw))
}

/// Returns the catalog for `region`, from cache when it is fresh unless `force` is set.
/// Falls back to a stale cache when the network is unavailable.
#[tauri::command]
async fn fetch_catalog(app: AppHandle, region: String, force: bool) -> Result<Catalog, String> {
    if !REGIONS.contains(&region.as_str()) {
        return Err(format!("Unknown region {region}"));
    }
    let cached = read_cache(&app, &region).await;
    if let Some(c) = &cached {
        let age = now_secs().saturating_sub(c.fetched_at);
        if !force && age < CACHE_MAX_AGE.as_secs() {
            let c = cached.unwrap();
            return Ok(Catalog { region, fetched_at: c.fetched_at, from_cache: true, games: c.games });
        }
    }
    match download(&app, &region).await {
        Ok(games) => {
            let cache = CacheFile { region: region.clone(), fetched_at: now_secs(), games };
            // A failed cache write should not hide a successful download.
            if let Err(e) = write_cache(&app, &cache).await {
                eprintln!("catalog cache write failed: {e}");
            }
            Ok(Catalog { region, fetched_at: cache.fetched_at, from_cache: false, games: cache.games })
        }
        Err(err) => match cached {
            Some(c) => Ok(Catalog { region, fetched_at: c.fetched_at, from_cache: true, games: c.games }),
            None => Err(err),
        },
    }
}

#[tauri::command]
fn regions() -> Vec<&'static str> {
    REGIONS.to_vec()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![fetch_catalog, regions])
        .run(tauri::generate_context!())
        .expect("error while running Switch Library");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_base_titles_and_dedupes() {
        let json = r#"{
            "1": {"id": "01007EF00011E000", "name": "Game A", "screenshots": ["a"]},
            "2": {"id": "01007ef00011e000", "name": "Game A", "description": "richer"},
            "3": {"id": "01007EF00011E800", "name": "Game A Update"},
            "4": {"id": "01007EF00011F001", "name": "Game A DLC"},
            "5": {"id": null, "name": "No id"},
            "6": {"id": "0100000000010000", "name": "  "}
        }"#;
        let raw: HashMap<String, RawTitle> = serde_json::from_str(json).unwrap();
        let games = slim(raw);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].id, "01007EF00011E000");
        assert_eq!(games[0].description.as_deref(), Some("richer"));
    }

    #[test]
    fn detects_demos_by_name() {
        assert!(looks_like_demo("The Legend of Zelda: Breath of the Wild Demo Version"));
        assert!(looks_like_demo("Game (Demo)"));
        assert!(!looks_like_demo("Demolition Derby"));
    }
}
