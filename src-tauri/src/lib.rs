use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use http::HeaderValue;
use scraper::{Html, Selector};
use serde::Serialize;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, Window};
use wreq::Method;

#[derive(Clone, Serialize)]
struct ProgState {
    progress: f64,
    downloaded: u64,
    total: u64,
    error: Option<String>,
    paused: bool,
    sp: Option<String>,
    status: Option<String>,
}

#[derive(PartialEq, Clone)]
enum CtrlState {
    Running,
    Paused,
    Cancelled,
}

static PROGRESS: LazyLock<Mutex<HashMap<String, ProgState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

static CTRL: LazyLock<Mutex<HashMap<String, CtrlState>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

static HTTP_CLIENT: LazyLock<wreq::Client> = LazyLock::new(|| {
    wreq::Client::builder()
        .emulation(wreq_util::Emulation::Firefox148)
        .build()
        .unwrap()
});

#[derive(Clone, Serialize)]
struct DownloadInfo {
    progress: f64,
    downloaded: u64,
    total: u64,
    error: Option<String>,
    paused: bool,
    status: Option<String>,
}

fn get_links_from_page(html: &str) -> Result<Vec<String>, String> {
    let document = Html::parse_document(html);

    let file_hoster = Selector::parse("div.entry-content ul > li:nth-child(2) > a")
        .map_err(|e| format!("Selector: {}", e))?;
    let tags: Vec<_> = document
        .select(&file_hoster)
        .filter(|t| {
            let text = t.text().collect::<String>();
            text.contains("Filehoster: FuckingFast")
        })
        .collect();

    if tags.is_empty() {
        return Err("no fuckingfast link found".into());
    }

    let href = tags[0]
        .attr("href")
        .ok_or("no href")?
        .to_string();

    let spoiler_sel = Selector::parse(
        "div.entry-content ul > li:nth-child(2) > div.su-spoiler > div.su-spoiler-content",
    )
    .map_err(|e| format!("Selector: {}", e))?;
    let spoilers = document.select(&spoiler_sel).collect::<Vec<_>>();

    if spoilers.is_empty() {
        return Ok(vec![href]);
    }

    let mut results = Vec::new();
    let link_sel = Selector::parse("a").map_err(|e| format!("Selector: {}", e))?;
    for spoiler in &spoilers {
        for link in spoiler.select(&link_sel) {
            if let Some(h) = link.attr("href") {
                results.push(h.to_string());
            }
        }
    }
    results.sort_by(|a, b| {
        let af = a.split('#').nth(1).unwrap_or(a);
        let bf = b.split('#').nth(1).unwrap_or(b);
        af.cmp(bf)
    });
    results.dedup();
    Ok(results)
}

async fn fetch_page(url: &str) -> Result<String, String> {
    let uri: http::Uri = url.parse().map_err(|e| format!("URI: {}", e))?;
    let req = wreq::Request::new(Method::GET, uri);
    let resp = HTTP_CLIENT
        .execute(req)
        .await
        .map_err(|e| format!("HTTP: {}", e))?;

    if resp.status() == 403 {
        return Err("Cloudflare/DDoS protection".into());
    }

    resp.text()
        .await
        .map_err(|e| format!("Body: {}", e))
}

#[cfg(target_os = "windows")]
fn native_click(x: i32, y: i32) {
    use std::thread::sleep;
    use std::time::Duration;
    extern "system" {
        fn SetCursorPos(X: i32, Y: i32) -> i32;
        fn mouse_event(dwFlags: u32, dx: i32, dy: i32, dwData: u32, dwExtraInfo: usize);
    }
    const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
    const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
    unsafe {
        SetCursorPos(x, y);
        sleep(Duration::from_millis(40));
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0);
        sleep(Duration::from_millis(60));
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0);
    }
}

const RESOLVER_JS: &str = r#"
(function () {
  if (window.__ff_resolver_started) return;
  window.__ff_resolver_started = true;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const dbg = (s) => {
    try {
      document.title = "FF|" + String(s).slice(0, 100);
    } catch (e) {}
  };
  const report = (url) => {
    try {
      document.title = "FF_RESOLVED|" + url;
    } catch (e) {}
  };
  (async () => {
    try { window.open = function () { return null; }; } catch (e) {}
    dbg("start");
    const clickTurnstile = async () => {
      try {
        const frames = document.querySelectorAll(
          "iframe[src*='challenges.cloudflare.com'], iframe[src*='challenges.cloudflare']"
        );
        for (const f of frames) {
          try {
            const rect = f.getBoundingClientRect();
            if (rect.width > 0 && rect.height > 0) {
              const uniq = Math.floor(Math.random() * 1e6);
              document.title = "FF_CLICK|" + Math.round(rect.left + 40) + "," + Math.round(rect.top + 32) + "," + uniq;
              return;
            }
          } catch (e) {}
        }
      } catch (e) {}
    };
    await clickTurnstile();
    let btn = null;
    const deadline = Date.now() + 120000;
    const selectors = [
      "a.gay-button",
      "button.gay-button",
      "a[class*='gay-button']",
      "a[href*='/f/']"
    ];
    while (Date.now() < deadline) {
      for (const sel of selectors) {
        const el = document.querySelector(sel);
        if (el) { btn = el; break; }
      }
      if (btn) {
        const html = btn.outerHTML || "";
        const disabled = /not-allowed|opacity\s*:\s*0\.[0-4]|disabled/i.test(html);
        if (!disabled) { dbg("btn_ready"); break; }
      }
      if (Date.now() % 6000 < 400) await clickTurnstile();
      await sleep(400);
    }
    if (!btn) { dbg("no_button"); return; }
    try { btn.click(); } catch (e) {}
    dbg("clicked");
    const dlDeadline = Date.now() + 20000;
    while (Date.now() < dlDeadline) {
      if ((document.cookie || "").indexOf("dlpass") !== -1) { dbg("dlpass_ok"); break; }
      await sleep(300);
    }
    const fileId = (location.pathname || "").replace(/^\//, "");
    const goUrl = "https://fuckingfast.co/f/" + fileId + "/go";
    for (let attempt = 0; attempt < 5; attempt++) {
      try {
        const resp = await fetch(goUrl, {
          method: "POST",
          headers: {
            "HX-Request": "true",
            "HX-Current-URL": location.href,
            "Origin": "https://fuckingfast.co",
            "Content-Type": "application/x-www-form-urlencoded"
          },
          body: ""
        });
        const hx = resp.headers.get("HX-Redirect") || resp.headers.get("hx-redirect");
        if (hx) {
          const url = hx.startsWith("/") ? "https://fuckingfast.co" + hx : hx;
          report(url);
          return;
        }
      } catch (e) {}
      await sleep(1000);
    }
    dbg("give_up");
  })();
})();
"#;

async fn wait_resolver(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    window: &tauri::WebviewWindow,
    link: &str,
) -> Result<String, String> {
    let t0 = tokio::time::Instant::now();
    let mut shown = false;
    let mut last_title = String::new();
    loop {
        let elapsed = t0.elapsed();
        if elapsed >= Duration::from_secs(360) {
            return Err("Timeout: could not solve Cloudflare".into());
        }
        if !shown && elapsed >= Duration::from_secs(15) {
            shown = true;
            let _ = window.show();
        }
        tokio::select! {
            v = rx.recv() => match v {
                Some(d) => return Ok(d),
                None => return Err("resolver closed".to_string()),
            },
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if let Ok(t) = window.title() {
                    if t != last_title {
                        last_title = t.clone();
                        if let Some(rest) = t.strip_prefix("FF_RESOLVED|") {
                            if !rest.is_empty() {
                                return Ok(rest.to_string());
                            }
                        } else if let Some(rest) = t.strip_prefix("FF_CLICK|") {
                            let parts: Vec<&str> = rest.split(',').collect();
                            if parts.len() >= 2 {
                                if let (Ok(x), Ok(y)) = (
                                    parts[0].trim().parse::<f64>(),
                                    parts[1].trim().parse::<f64>(),
                                ) {
                                    let _ = window.show();
                                    let scale = window.scale_factor().unwrap_or(1.0);
                                    let pos = window.outer_position().unwrap_or_default();
                                    let sx = (pos.x as f64 + x * scale) as i32;
                                    let sy = (pos.y as f64 + y * scale) as i32;
                                    std::thread::spawn(move || native_click(sx, sy));
                                }
                            }
                        } else if let Some(rest) = t.strip_prefix("FF|") {
                            let msg = human_status(rest.trim()).to_string();
                            PROGRESS
                                .lock()
                                .unwrap()
                                .get_mut(link)
                                .map(|s| s.status = Some(msg));
                        }
                    }
                }
                if check_ctrl(link) == CtrlState::Cancelled {
                    return Err("Cancelled".into());
                }
            }
        }
    }
}

async fn resolve_via_webview(app: &AppHandle, link: &str) -> Result<String, String> {
    let url = Url::parse(link).map_err(|e| format!("URL parse: {}", e))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let label = format!(
        "ff_resolver_{}_{}",
        url.path().trim_start_matches('/'),
        stamp
    );

    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.close();
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .visible(false)
        .title("Solving Cloudflare...")
        .decorations(false)
        .inner_size(560.0, 700.0)
        .initialization_script(RESOLVER_JS)
        .on_navigation({
            let tx = tx.clone();
            move |u| {
                if let Some(host) = u.host_str() {
                    if host == "dl.fuckingfast.co" || host.ends_with(".fuckingfast.co") {
                        let _ = tx.send(u.to_string());
                        return false;
                    }
                }
                true
            }
        })
        .build()
        .map_err(|e| format!("resolver window: {}", e))?;

    let result = wait_resolver(&mut rx, &window, link).await;
    let _ = window.close();
    result
}

async fn resolve_download_url(app: &AppHandle, link: &str) -> Result<(String, String), String> {
    let filename = link
        .split('#')
        .nth(1)
        .ok_or("no filename")?
        .to_string();

    let dl_url = resolve_via_webview(app, link).await?;

    Ok((dl_url, filename))
}

fn check_ctrl(link: &str) -> CtrlState {
    CTRL
        .lock()
        .unwrap()
        .get(link)
        .cloned()
        .unwrap_or(CtrlState::Running)
}

#[tauri::command]
async fn get_links(url: String) -> Result<Vec<String>, String> {
    let html = fetch_page(&url).await?;
    get_links_from_page(&html)
}

#[tauri::command]
async fn resolve_dl_url(app: AppHandle, link: String) -> Result<(String, String), String> {
    resolve_download_url(&app, &link).await
}

#[tauri::command]
async fn pause_download(link: String) -> Result<(), String> {
    CTRL.lock().unwrap().insert(link, CtrlState::Paused);
    Ok(())
}

#[tauri::command]
async fn resume_download(link: String) -> Result<(), String> {
    CTRL
        .lock()
        .unwrap()
        .insert(link.clone(), CtrlState::Running);
    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.paused = false);
    Ok(())
}

#[tauri::command]
async fn cancel_download(link: String) -> Result<(), String> {
    CTRL
        .lock()
        .unwrap()
        .insert(link.clone(), CtrlState::Cancelled);
    let path = PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| {
            s.error = Some("Cancelled".into());
            s.sp.clone()
        })
        .flatten();
    // Parts may still hold the file handle; retry a few times so the file is
    // eventually removed after cancel is clicked.
    if let Some(p) = path {
        tokio::spawn(async move {
            for _ in 0..120 {
                if tokio::fs::remove_file(&p).await.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }
    Ok(())
}

const SYNC_INTERVAL: u64 = 256 * 1024;

const MAX_RETRIES: u32 = 5;

const IDLE_TIMEOUT: Duration = Duration::from_secs(20);

async fn probe_total(dl_url: &str) -> u64 {
    let uri: http::Uri = match dl_url.parse() {
        Ok(u) => u,
        Err(_) => return 0,
    };
    let mut req = wreq::Request::new(Method::GET, uri);
    req.headers_mut()
        .insert("Range", HeaderValue::from_static("bytes=0-0"));
    let resp = match HTTP_CLIENT.execute(req).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if resp.status() != 206 {
        return 0;
    }
    match resp.headers().get("Content-Range") {
        Some(cr) => {
            let v = cr
                .to_str()
                .ok()
                .and_then(|s| s.rsplit('/').next()?.parse::<u64>().ok())
                .unwrap_or(0);
            v
        }
        None => 0,
    }
}

async fn download_part(
    link: String,
    dl_url: String,
    sp: String,
    start: u64,
    end: u64,
) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

    let part_len = end - start + 1;
    let mut written: u64 = 0;
    let mut last_sync: u64 = 0;

    for attempt in 0..=MAX_RETRIES {
        let from = start + written;
        if from > end {
            return Ok(());
        }

        loop {
            match check_ctrl(&link) {
                CtrlState::Cancelled => return Err("Cancelled".into()),
                CtrlState::Paused => {
                    PROGRESS
                        .lock()
                        .unwrap()
                        .get_mut(&link)
                        .map(|s| s.paused = true);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                CtrlState::Running => break,
            }
        }

        let uri: http::Uri = dl_url.parse().map_err(|e| format!("URI: {}", e))?;
        let mut req = wreq::Request::new(Method::GET, uri);
        req.headers_mut().insert(
            "Range",
            HeaderValue::try_from(format!("bytes={from}-{end}"))
                .map_err(|e| format!("Range: {}", e))?,
        );
        let resp = HTTP_CLIENT
            .execute(req)
            .await
            .map_err(|e| format!("HTTP: {}", e))?;

        let ranged = resp.status() == 206;
        if !ranged && !resp.status().is_success() {
            if attempt == MAX_RETRIES {
                return Err(format!("HTTP {}", resp.status()));
            }
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            continue;
        }
        if !ranged && from > 0 {
            if attempt == MAX_RETRIES {
                return Err("Server does not support range requests".into());
            }
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            continue;
        }

        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&sp)
            .await
            .map_err(|e| format!("Open: {}", e))?;
        AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(from))
            .await
            .map_err(|e| format!("Seek: {}", e))?;

        let mut stream = resp.bytes_stream();
        let mut fail: Option<String> = None;

        // Per-chunk idle timeout: if the connection accepts but no data arrives
        // (CDN keeps the connection open), do not let the part wait forever; retry.
        loop {
            let item = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await;
            let item = match item {
                Ok(Some(i)) => i,
                Ok(None) => break,
                Err(_) => {
                    fail = Some("Idle timeout: no data received".into());
                    break;
                }
            };
            let chunk = match item {
                Ok(c) => c,
                Err(e) => {
                    fail = Some(format!("Stream: {}", e));
                    break;
                }
            };

            if let Err(e) = AsyncWriteExt::write_all(&mut f, &chunk).await {
                fail = Some(format!("Write: {}", e));
                break;
            }

            written += chunk.len() as u64;

            if written - last_sync < SYNC_INTERVAL && written < part_len {
                continue;
            }
            let delta = written - last_sync;
            last_sync = written;

            loop {
                match check_ctrl(&link) {
                    CtrlState::Cancelled => return Err("Cancelled".into()),
                    CtrlState::Paused => {
                        PROGRESS
                            .lock()
                            .unwrap()
                            .get_mut(&link)
                            .map(|s| s.paused = true);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                    CtrlState::Running => break,
                }
            }

            PROGRESS.lock().unwrap().get_mut(&link).map(|s| {
                s.downloaded += delta;
                if s.total > 0 {
                    s.progress = (s.downloaded as f64 / s.total as f64) * 100.0;
                }
            });
        }
        drop(f);

        if written < part_len {
            let e = fail.unwrap_or_else(|| "Missing data (connection closed)".into());
            if attempt == MAX_RETRIES {
                return Err(e);
            }
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            continue;
        }

        return Ok(());
    }
    Ok(())
}

async fn parallel_download(link: String, dl_url: String, sp: String, total: u64, parts: u64) {
    let file = match tokio::fs::File::create(&sp).await {
        Ok(f) => f,
        Err(e) => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("File: {}", e)));
            return;
        }
    };
    // Pre-allocate the file so parts can write to independent offsets.
    if let Err(e) = file.set_len(total).await {
        PROGRESS
            .lock()
            .unwrap()
            .get_mut(&link)
            .map(|s| s.error = Some(format!("SetLen: {}", e)));
        return;
    }
    drop(file);
    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.total = total);

    let mut parts = parts;
    if parts > total {
        parts = total;
    }
    let part_len = total / parts;

    let mut handles = Vec::new();
    for i in 0..parts {
        let start = i * part_len;
        let end = if i == parts - 1 {
            total - 1
        } else {
            (i + 1) * part_len - 1
        };
        let link_c = link.clone();
        let dl_url_c = dl_url.clone();
        let sp_c = sp.clone();
        handles.push(tokio::spawn(async move {
            download_part(link_c, dl_url_c, sp_c, start, end).await
        }));
    }

    let mut any_err: Option<String> = None;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if any_err.is_none() {
                    any_err = Some(e);
                }
            }
            Err(_) => {
                if any_err.is_none() {
                    any_err = Some("part task panicked".into());
                }
            }
        }
    }

    match any_err {
        Some(e) => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(e.clone()));
            if check_ctrl(&link) == CtrlState::Cancelled || e == "Cancelled" {
                let _ = tokio::fs::remove_file(&sp).await;
            }
        }
        None => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.progress = 100.0);
        }
    }
}

async fn single_download(link: String, dl_url: String, sp: String, total: u64) {
    let uri: http::Uri = match dl_url.parse() {
        Ok(u) => u,
        Err(e) => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("URI: {}", e)));
            return;
        }
    };

    let req = wreq::Request::new(Method::GET, uri);
    let resp = match HTTP_CLIENT.execute(req).await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("HTTP {}", r.status())));
            return;
        }
        Err(e) => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("HTTP: {}", e)));
            return;
        }
    };

    let mut file = match tokio::fs::File::create(&sp).await {
        Ok(f) => f,
        Err(e) => {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("File: {}", e)));
            return;
        }
    };

    let mut downloaded: u64 = 0;
    let mut last_sync: u64 = 0;
    let mut stream = resp.bytes_stream();

    use futures_util::StreamExt;
    loop {
        let timed = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await;
        let item = match timed {
            Ok(Some(i)) => i,
            Ok(None) => break,
            Err(_) => {
                PROGRESS
                    .lock()
                    .unwrap()
                    .get_mut(&link)
                    .map(|s| s.error = Some("Idle timeout: no data received".into()));
                return;
            }
        };
        let chunk = match item {
            Ok(c) => c,
            Err(e) => {
                PROGRESS
                    .lock()
                    .unwrap()
                    .get_mut(&link)
                    .map(|s| s.error = Some(format!("Stream: {}", e)));
                return;
            }
        };

        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("Write: {}", e)));
            return;
        }

        downloaded += chunk.len() as u64;

        if downloaded - last_sync < SYNC_INTERVAL && downloaded != total {
            continue;
        }
        last_sync = downloaded;

        loop {
            match check_ctrl(&link) {
                CtrlState::Cancelled => {
                    let _ = tokio::fs::remove_file(&sp).await;
                    PROGRESS
                        .lock()
                        .unwrap()
                        .get_mut(&link)
                        .map(|s| s.error = Some("Cancelled".into()));
                    return;
                }
                CtrlState::Paused => {
                    PROGRESS
                        .lock()
                        .unwrap()
                        .get_mut(&link)
                        .map(|s| s.paused = true);
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                }
                CtrlState::Running => break,
            }
        }

        PROGRESS.lock().unwrap().get_mut(&link).map(|s| {
            s.downloaded = downloaded;
            if total > 0 {
                s.progress = (downloaded as f64 / total as f64) * 100.0;
            }
        });
    }

    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.progress = 100.0);
}

fn human_status(code: &str) -> &str {
    match code {
        "start" => "Checking Cloudflare...",
        "btn_ready" => "Download button ready, clicking...",
        "clicked" => "Button clicked, waiting...",
        "dlpass_ok" => "Session cookie received, resolving link...",
        "no_button" => "Download button not found!",
        "give_up" => "Resolution failed!",
        _ => "Resolving link...",
    }
}

#[tauri::command]
async fn start_download(app: AppHandle, link: String, save_dir: String, parts: u64) -> Result<String, String> {
    PROGRESS.lock().unwrap().insert(
        link.clone(),
        ProgState {
            progress: 0.0,
            downloaded: 0,
            total: 0,
            error: None,
            paused: false,
            sp: None,
            status: Some("Resolving link (Cloudflare)...".into()),
        },
    );
    CTRL.lock().unwrap().insert(link.clone(), CtrlState::Running);

    let (dl_url, filename) = match resolve_download_url(&app, &link).await {
        Ok(v) => v,
        Err(e) => return Err(e),
    };
    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.status = Some("Preparing download...".into()));
    let save_path = std::path::Path::new(&save_dir).join(&filename);
    let sp = save_path.to_string_lossy().to_string();
    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.sp = Some(sp.clone()));

    let link_c = link.clone();
    tokio::spawn(async move {
        let total = probe_total(&dl_url).await;
        let parts = parts.clamp(1, 16);
        if total == 0 || parts <= 1 {
            single_download(link_c, dl_url, sp, total).await;
        } else {
            parallel_download(link_c, dl_url, sp, total, parts).await;
        }
    });
    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.status = None);

    Ok(filename)
}

#[tauri::command]
async fn get_download_progress(link: String) -> Result<DownloadInfo, String> {
    let m = PROGRESS.lock().unwrap();
    if let Some(s) = m.get(&link) {
        Ok(DownloadInfo {
            progress: s.progress,
            downloaded: s.downloaded,
            total: s.total,
            error: s.error.clone(),
            paused: s.paused,
            status: s.status.clone(),
        })
    } else {
        Ok(DownloadInfo {
            progress: 0.0,
            downloaded: 0,
            total: 0,
            error: None,
            paused: false,
            status: None,
        })
    }
}

#[tauri::command]
async fn clear_download(link: String) -> Result<(), String> {
    PROGRESS.lock().unwrap().remove(&link);
    CTRL.lock().unwrap().remove(&link);
    Ok(())
}

#[tauri::command]
fn window_minimize(window: Window) -> Result<bool, String> {
    window.minimize().map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn window_toggle_maximize(window: Window) -> Result<bool, String> {
    if window.is_maximized().map_err(|e| e.to_string())? {
        window.unmaximize().map_err(|e| e.to_string())?;
    } else {
        window.maximize().map_err(|e| e.to_string())?;
    }
    Ok(true)
}

#[tauri::command]
fn window_close(window: Window) -> Result<bool, String> {
    window.close().map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_links,
            resolve_dl_url,
            start_download,
            get_download_progress,
            clear_download,
            pause_download,
            resume_download,
            cancel_download,
            window_minimize,
            window_toggle_maximize,
            window_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}