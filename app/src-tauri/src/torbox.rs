//! Minimal TorBox client: log in with an API key, add a magnet or web link,
//! list transfers with their status, and download finished ones to disk.
//!
//! The API key stays in the Rust process and in a file in the app config
//! folder; the webview never sees it after login.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;

const API: &str = "https://api.torbox.app/v1/api";

#[derive(Default)]
pub struct TorBoxState {
    key: Mutex<Option<String>>,
    downloading: Mutex<HashSet<String>>,
}

#[derive(Serialize, Deserialize)]
struct SavedKey {
    api_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    email: Option<String>,
    plan: Option<String>,
    premium_expires_at: Option<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Torrent,
    Web,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Torrent => "torrents",
            Kind::Web => "webdl",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    kind: Kind,
    id: u64,
    name: String,
    state: String,
    progress: f64,
    size: u64,
    speed: u64,
    eta: u64,
    ready: bool,
    file_count: usize,
    created_at: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadEvent {
    key: String,
    received: u64,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
    path: Option<String>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("switch-library/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())
}

fn key_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("torbox.json"))
}

fn current_key(state: &TorBoxState) -> Result<String, String> {
    state.key.lock().unwrap().clone().ok_or_else(|| "Not logged in to TorBox".to_string())
}

/// Every TorBox response is `{ success, error, detail, data }`. Return `data` or a readable error.
async fn unwrap_response(resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let ok = body.get("success").and_then(Value::as_bool).unwrap_or(status.is_success());
    if status.is_success() && ok {
        return Ok(body.get("data").cloned().unwrap_or(Value::Null));
    }
    let detail = body
        .get("detail")
        .and_then(|d| d.as_str().map(str::to_string).or_else(|| Some(d.to_string())))
        .filter(|s| s != "null")
        .or_else(|| body.get("error").and_then(Value::as_str).map(str::to_string));
    Err(match (status.as_u16(), detail) {
        (401 | 403, _) => "TorBox rejected the API key".to_string(),
        (_, Some(d)) => d,
        (code, None) => format!("TorBox returned HTTP {code}"),
    })
}

async fn get(key: &str, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
    let resp = client()?
        .get(format!("{API}/{path}"))
        .bearer_auth(key)
        .query(query)
        .send()
        .await
        .map_err(|e| format!("Could not reach TorBox: {e}"))?;
    unwrap_response(resp).await
}

async fn fetch_account(key: &str) -> Result<Account, String> {
    let d = get(key, "user/me", &[]).await?;
    let plan = d.get("plan").and_then(Value::as_u64).map(|p| {
        match p {
            0 => "Free",
            1 => "Essential",
            2 => "Pro",
            3 => "Standard",
            _ => "Paid",
        }
        .to_string()
    });
    Ok(Account {
        email: d.get("email").and_then(Value::as_str).map(str::to_string),
        plan,
        premium_expires_at: d.get("premium_expires_at").and_then(Value::as_str).map(str::to_string),
    })
}

#[tauri::command]
pub async fn torbox_login(app: AppHandle, state: State<'_, TorBoxState>, api_key: String) -> Result<Account, String> {
    let key = api_key.trim().to_string();
    if key.is_empty() {
        return Err("Enter your TorBox API key".into());
    }
    let account = fetch_account(&key).await?;
    let path = key_file(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_vec(&SavedKey { api_key: key.clone() }).unwrap())
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    *state.key.lock().unwrap() = Some(key);
    Ok(account)
}

/// Restores a saved key at startup. Returns `None` when there is no key.
#[tauri::command]
pub async fn torbox_session(app: AppHandle, state: State<'_, TorBoxState>) -> Result<Option<Account>, String> {
    let saved = key_file(&app)
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<SavedKey>(&b).ok());
    let Some(saved) = saved else { return Ok(None) };
    *state.key.lock().unwrap() = Some(saved.api_key.clone());
    fetch_account(&saved.api_key).await.map(Some)
}

#[tauri::command]
pub fn torbox_logout(app: AppHandle, state: State<'_, TorBoxState>) -> Result<(), String> {
    *state.key.lock().unwrap() = None;
    if let Ok(p) = key_file(&app) {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

fn classify(link: &str) -> Option<Kind> {
    let l = link.trim();
    if l.starts_with("magnet:?") {
        Some(Kind::Torrent)
    } else if l.starts_with("https://") || l.starts_with("http://") {
        Some(Kind::Web)
    } else {
        None
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    kind: Kind,
    id: Option<u64>,
}

#[tauri::command]
pub async fn torbox_add(state: State<'_, TorBoxState>, link: String) -> Result<Added, String> {
    add_link(&current_key(&state)?, &link).await
}

async fn add_link(key: &str, link: &str) -> Result<Added, String> {
    let link = link.trim().to_string();
    let kind = classify(&link).ok_or("Paste a magnet link or an http(s) URL")?;
    let req = match kind {
        Kind::Torrent => client()?
            .post(format!("{API}/torrents/createtorrent"))
            .multipart(reqwest::multipart::Form::new().text("magnet", link)),
        Kind::Web => client()?
            .post(format!("{API}/webdl/createwebdownload"))
            .form(&[("link", link)]),
    };
    let resp = req.bearer_auth(key).send().await.map_err(|e| format!("Could not reach TorBox: {e}"))?;
    let data = unwrap_response(resp).await.map_err(|e| {
        if kind == Kind::Web && e.contains("not supported") {
            format!("{e} TorBox only accepts links from its supported hosts (see torbox.app → Web Downloads).")
        } else {
            e
        }
    })?;
    let id = ["torrent_id", "webdownload_id", "web_id", "id"]
        .iter()
        .find_map(|k| data.get(*k).and_then(Value::as_u64));
    Ok(Added { kind, id })
}

fn num(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f.max(0.0) as u64))).unwrap_or(0)
}

fn to_transfer(kind: Kind, v: &Value) -> Option<Transfer> {
    let id = v.get("id")?.as_u64()?;
    let finished = v.get("download_finished").and_then(Value::as_bool).unwrap_or(false);
    let present = v.get("download_present").and_then(Value::as_bool).unwrap_or(false);
    let mut progress = v.get("progress").and_then(Value::as_f64).unwrap_or(0.0);
    if progress > 1.0 {
        progress /= 100.0; // tolerate percentages
    }
    if finished || present {
        progress = 1.0;
    }
    Some(Transfer {
        kind,
        id,
        name: v.get("name").and_then(Value::as_str).unwrap_or("Untitled").to_string(),
        state: v.get("download_state").and_then(Value::as_str).unwrap_or("unknown").to_string(),
        progress,
        size: num(v, "size"),
        speed: num(v, "download_speed"),
        eta: num(v, "eta"),
        ready: finished && present,
        file_count: v.get("files").and_then(Value::as_array).map_or(0, Vec::len),
        created_at: v.get("created_at").and_then(Value::as_str).map(str::to_string),
    })
}

#[tauri::command]
pub async fn torbox_list(state: State<'_, TorBoxState>) -> Result<Vec<Transfer>, String> {
    list_transfers(&current_key(&state)?).await
}

async fn list_transfers(key: &str) -> Result<Vec<Transfer>, String> {
    let q = [("bypass_cache", "true".to_string())];
    let (t, w) = tokio::join!(get(key, "torrents/mylist", &q), get(key, "webdl/mylist", &q));
    let mut out = Vec::new();
    for (kind, res) in [(Kind::Torrent, t), (Kind::Web, w)] {
        if let Some(items) = res?.as_array() {
            out.extend(items.iter().filter_map(|v| to_transfer(kind, v)));
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    Ok(out)
}

#[tauri::command]
pub async fn torbox_remove(state: State<'_, TorBoxState>, kind: Kind, id: u64) -> Result<(), String> {
    remove_item(&current_key(&state)?, kind, id).await
}

async fn remove_item(key: &str, kind: Kind, id: u64) -> Result<(), String> {
    let (path, id_field) = match kind {
        Kind::Torrent => ("torrents/controltorrent", "torrent_id"),
        Kind::Web => ("webdl/controlwebdownload", "webdl_id"),
    };
    let resp = client()?
        .post(format!("{API}/{path}"))
        .bearer_auth(key)
        .json(&serde_json::json!({ "operation": "delete", id_field: id }))
        .send()
        .await
        .map_err(|e| format!("Could not reach TorBox: {e}"))?;
    unwrap_response(resp).await.map(|_| ())
}

/// A direct download URL for a finished item; a zip when it has several files.
async fn request_link(key: &str, kind: Kind, id: u64, file_count: usize) -> Result<String, String> {
    let id_param = match kind {
        Kind::Torrent => "torrent_id",
        Kind::Web => "web_id",
    };
    let mut q = vec![("token", key.to_string()), (id_param, id.to_string())];
    if file_count != 1 {
        q.push(("zip_link", "true".into()));
    }
    get(key, &format!("{}/requestdl", kind.path()), &q)
        .await?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "TorBox did not return a download link".to_string())
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() { "download".into() } else { trimmed.chars().take(180).collect() }
}

fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (1..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

fn filename_from_disposition(h: &str) -> Option<String> {
    let part = h.split(';').map(str::trim).find(|p| p.starts_with("filename="))?;
    Some(part.trim_start_matches("filename=").trim_matches('"').to_string()).filter(|s| !s.is_empty())
}

/// Asks TorBox for a download link (a zip when there are several files) and
/// streams it into the user's Downloads folder, emitting `torbox-download` events.
#[tauri::command]
pub async fn torbox_download(
    app: AppHandle,
    state: State<'_, TorBoxState>,
    kind: Kind,
    id: u64,
    name: String,
    file_count: usize,
) -> Result<(), String> {
    let key = current_key(&state)?;
    let dl_key = format!("{}-{id}", kind.path());
    if !state.downloading.lock().unwrap().insert(dl_key.clone()) {
        return Err("Already downloading".into());
    }

    let result = async {
        let url = request_link(&key, kind, id, file_count).await?;

        let resp = client()?
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("Download failed: {e}"))?;
        let total = resp.content_length();
        let fname = resp
            .headers()
            .get(reqwest::header::CONTENT_DISPOSITION)
            .and_then(|h| h.to_str().ok())
            .and_then(filename_from_disposition)
            .unwrap_or_else(|| if file_count != 1 { format!("{name}.zip") } else { name.clone() });

        let dir = app.path().download_dir().map_err(|e| e.to_string())?;
        tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
        let dest = unique_path(&dir, &sanitize(&fname));
        let part = dest.with_extension(format!(
            "{}part",
            dest.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()
        ));

        let mut file = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
        let mut stream = resp.bytes_stream();
        let (mut received, mut last) = (0u64, 0u64);
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(format!("Download interrupted: {e}"));
                }
            };
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
            received += chunk.len() as u64;
            if received - last > 1024 * 1024 {
                last = received;
                let _ = app.emit(
                    "torbox-download",
                    DownloadEvent { key: dl_key.clone(), received, total, done: false, error: None, path: None },
                );
            }
        }
        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);
        tokio::fs::rename(&part, &dest).await.map_err(|e| e.to_string())?;
        Ok::<_, String>((received, total, dest))
    }
    .await;

    state.downloading.lock().unwrap().remove(&dl_key);
    let event = match &result {
        Ok((received, total, dest)) => DownloadEvent {
            key: dl_key,
            received: *received,
            total: *total,
            done: true,
            error: None,
            path: Some(dest.to_string_lossy().into_owned()),
        },
        Err(e) => DownloadEvent { key: dl_key, received: 0, total: None, done: true, error: Some(e.clone()), path: None },
    };
    let _ = app.emit("torbox-download", event);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live round trip against a real account. Run with
    /// `TORBOX_API_KEY=... cargo test -- --ignored live --nocapture`.
    /// Adds a small public test file as a web download, waits for it,
    /// fetches the first bytes of the download link, then removes it.
    #[tokio::test]
    #[ignore]
    async fn live_round_trip() {
        let key = std::env::var("TORBOX_API_KEY").expect("set TORBOX_API_KEY");
        let account = fetch_account(&key).await.expect("login");
        println!("account plan={:?} expires={:?}", account.plan, account.premium_expires_at);

        let link = std::env::var("TORBOX_TEST_LINK")
            .unwrap_or_else(|_| "https://proof.ovh.net/files/1Mb.dat".to_string());
        let added = add_link(&key, &link).await.expect("add");
        println!("added web={} id={:?}", added.kind == Kind::Web, added.id);
        let id = added.id.expect("id in create response");

        let mut ready = None;
        for _ in 0..30 {
            let list = list_transfers(&key).await.expect("list");
            if let Some(t) = list.iter().find(|t| t.kind == added.kind && t.id == id) {
                println!("state={} progress={:.2} ready={} files={} size={}", t.state, t.progress, t.ready, t.file_count, t.size);
                if t.ready {
                    ready = Some(t.file_count);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        let file_count = ready.expect("became ready within 90s");
        let url = request_link(&key, added.kind, id, file_count).await.expect("requestdl");
        let resp = client().unwrap().get(&url).header("Range", "bytes=0-1023").send().await.expect("fetch");
        println!("download status={} disposition={:?}", resp.status(), resp.headers().get("content-disposition"));
        assert!(resp.status().is_success());

        remove_item(&key, added.kind, id).await.expect("remove");
        let list = list_transfers(&key).await.expect("list after remove");
        assert!(!list.iter().any(|t| t.kind == added.kind && t.id == id));
        println!("removed ok");
    }

    #[test]
    fn classifies_links() {
        assert!(classify("magnet:?xt=urn:btih:abc") == Some(Kind::Torrent));
        assert!(classify(" https://example.com/file.zip ") == Some(Kind::Web));
        assert!(classify("ftp://x").is_none());
    }

    #[test]
    fn parses_transfer() {
        let v = serde_json::json!({
            "id": 7, "name": "thing", "download_state": "downloading", "progress": 0.42,
            "size": 1000, "download_speed": 50, "eta": 12, "download_finished": false,
            "download_present": false, "files": [{}, {}], "created_at": "2026-01-01T00:00:00Z"
        });
        let t = to_transfer(Kind::Torrent, &v).unwrap();
        assert_eq!((t.id, t.file_count, t.ready), (7, 2, false));
        assert!((t.progress - 0.42).abs() < 1e-9);
        let done = serde_json::json!({ "id": 8, "download_finished": true, "download_present": true });
        assert!(to_transfer(Kind::Web, &done).unwrap().ready);
    }

    #[test]
    fn makes_safe_unique_names() {
        assert_eq!(sanitize("a/b:c?.zip"), "a_b_c_.zip");
        assert_eq!(sanitize("..."), "download");
        assert_eq!(filename_from_disposition(r#"attachment; filename="x y.zip""#).as_deref(), Some("x y.zip"));
    }
}
