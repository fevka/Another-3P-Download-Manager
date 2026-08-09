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
        return Err("fuckingfast linki bulunamadı".into());
    }

    let href = tags[0]
        .attr("href")
        .ok_or("href yok")?
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
        return Err("Cloudflare/DDoS koruması".into());
    }

    resp.text()
        .await
        .map_err(|e| format!("Body: {}", e))
}

fn debug_log(msg: &str) {
    let path = std::env::temp_dir().join("ff_debug.txt");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = std::io::Write::write_all(&mut f, format!("[{}] {}\n", chrono_now(), msg).as_bytes());
    }
}

fn chrono_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
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
              const doc = f.contentDocument;
              if (doc) {
                const cb = doc.querySelector("input[type='checkbox'], .cf-turnstile input, label[role='checkbox']");
                if (cb) { cb.click(); dbg("turnstile_clicked"); }
              }
            }
          } catch (e) {}
        }
        const widget = document.querySelector(".cf-turnstile");
        if (widget) {
          const box = widget.querySelector("iframe");
          if (box) {
            const rect = box.getBoundingClientRect();
            box.dispatchEvent(new MouseEvent("click", {
              bubbles: true, cancelable: true, view: window,
              clientX: rect.left + rect.width / 2,
              clientY: rect.top + rect.height / 2
            }));
            dbg("turnstile_box_click");
          }
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
        dbg("fetch_" + resp.status + (hx ? "_hx" : "_nohx"));
        if (hx) {
          const url = hx.startsWith("/") ? "https://fuckingfast.co" + hx : hx;
          report(url);
          return;
        }
      } catch (e) {
        dbg("fetch_err_" + String(e).slice(0, 60));
      }
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
            return Err("Zaman aşımı: Cloudflare çözülemedi".into());
        }
        if !shown && elapsed >= Duration::from_secs(180) {
            shown = true;
            let _ = window.show();
            debug_log("timeout - pencere görünür yapıldı, manuel çözüm bekleniyor");
        }
        tokio::select! {
            v = rx.recv() => match v {
                Some(d) => {
                    if shown {
                        debug_log(&format!("resolved (manual): {}", d));
                    }
                    return Ok(d);
                }
                None => return Err("resolver kapatıldı".to_string()),
            },
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if let Ok(t) = window.title() {
                    if t != last_title {
                        last_title = t.clone();
                        debug_log(&format!("resolver title: {}", t));
                        if let Some(rest) = t.strip_prefix("FF_RESOLVED|") {
                            if !rest.is_empty() {
                                return Ok(rest.to_string());
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
    debug_log(&format!("resolve_via_webview: {}", link));
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
        .title("Cloudflare Çözülüyor...")
        .inner_size(560.0, 700.0)
        .initialization_script(RESOLVER_JS)
        .on_navigation({
            let tx = tx.clone();
            move |u| {
                debug_log(&format!("nav: {}", u));
                if let Some(host) = u.host_str() {
                    if host == "ff-dbg.local" {
                        let msg = u
                            .query_pairs()
                            .find(|(k, _)| k == "m")
                            .map(|(_, v)| v.to_string())
                            .unwrap_or_default();
                        debug_log(&format!("resolver: {}", msg));
                        return false;
                    }
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
        .ok_or("filename yok")?
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
    // Parçalar hâlâ dosya handle'ını tutuyor olabilir; handle kapanınca silmek için
    // birkaç kez dene. Bu sayede cancel'a tıklanınca dosya eninde sonunda kaldırılır.
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
    debug_log(&format!("probe: {}", dl_url));
    let uri: http::Uri = match dl_url.parse() {
        Ok(u) => u,
        Err(e) => {
            debug_log(&format!("probe URI hata: {}", e));
            return 0;
        }
    };
    let mut req = wreq::Request::new(Method::GET, uri);
    req.headers_mut()
        .insert("Range", HeaderValue::from_static("bytes=0-0"));
    let resp = match HTTP_CLIENT.execute(req).await {
        Ok(r) => r,
        Err(e) => {
            debug_log(&format!("probe HTTP hata: {}", e));
            return 0;
        }
    };
    if resp.status() != 206 {
        debug_log(&format!("probe status: {} (206 degil)", resp.status()));
        return 0;
    }
    match resp.headers().get("Content-Range") {
        Some(cr) => {
            let v = cr
                .to_str()
                .ok()
                .and_then(|s| s.rsplit('/').next()?.parse::<u64>().ok())
                .unwrap_or(0);
            debug_log(&format!("probe total: {} B", v));
            v
        }
        None => {
            debug_log("probe Content-Range yok");
            0
        }
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
            debug_log(&format!(
                "part [{start}-{end}] HTTP {} (attempt {})",
                resp.status(),
                attempt
            ));
            if attempt == MAX_RETRIES {
                return Err(format!("HTTP {}", resp.status()));
            }
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            continue;
        }
        if !ranged && from > 0 {
            if attempt == MAX_RETRIES {
                return Err("Sunucu aralık isteğini desteklemiyor".into());
            }
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            continue;
        }

        let mut f = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&sp)
            .await
            .map_err(|e| {
                debug_log(&format!("part open hata: {} sp={}", e, sp));
                format!("Open: {}", e)
            })?;
        AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(from))
            .await
            .map_err(|e| format!("Seek: {}", e))?;

        let mut stream = resp.bytes_stream();
        let mut fail: Option<String> = None;

        // Her veri parçası için idle timeout: bağlantı kabul edilip veri gelmezse
        // (CDN bağlantıyı açık tutarsa) parça sonsuza dek beklemesin; retry devreye girsin.
        loop {
            let item = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await;
            let item = match item {
                Ok(Some(i)) => i,
                Ok(None) => break,
                Err(_) => {
                    fail = Some("İdare timeout: veri gelmedi".into());
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
            let e = fail.unwrap_or_else(|| "Eksik veri (bağlantı kapandı)".into());
            if attempt == MAX_RETRIES {
                return Err(e);
            }
            debug_log(&format!(
                "retry part [{start}-{end}] from {from} attempt {}: {e}",
                attempt + 1
            ));
            tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            continue;
        }

        return Ok(());
    }
    Ok(())
}

async fn parallel_download(link: String, dl_url: String, sp: String, total: u64, parts: u64) {
    debug_log(&format!("parallel: sp={}", sp));
    let file = match tokio::fs::File::create(&sp).await {
        Ok(f) => f,
        Err(e) => {
            debug_log(&format!("parallel File::create hata: {}", e));
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("File: {}", e)));
            return;
        }
    };
    // Dosyayı önceden boyutlandır ki parçalar bağımsız offset'lere yazsın.
    if let Err(e) = file.set_len(total).await {
        debug_log(&format!("set_len hata: {}", e));
        PROGRESS
            .lock()
            .unwrap()
            .get_mut(&link)
            .map(|s| s.error = Some(format!("SetLen: {}", e)));
        return;
    }
    drop(file);
    debug_log(&format!("dosya hazir: {} ({} B)", sp, total));
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

    let sampler_link = link.clone();
    let sampler = tokio::spawn(async move {
        let mut prev: u64 = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let (cur, tot) = {
                let m = PROGRESS.lock().unwrap();
                match m.get(&sampler_link) {
                    Some(s) => (s.downloaded, s.total),
                    None => (0u64, 0u64),
                }
            };
            let speed = if cur >= prev {
                (cur - prev) as f64 / 2.0
            } else {
                0.0
            };
            debug_log(&format!(
                "speed_sample: {:.0} B/s delta={}B {}/{}B",
                speed,
                cur.saturating_sub(prev),
                cur,
                tot
            ));
            prev = cur;
            if tot > 0 && cur >= tot {
                break;
            }
        }
    });

    let mut any_err: Option<String> = None;
    for h in handles {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug_log(&format!("part HATA: {}", e));
                if any_err.is_none() {
                    any_err = Some(e);
                }
            }
            Err(_) => {
                debug_log("part panicked");
                if any_err.is_none() {
                    any_err = Some("part task panicked".into());
                }
            }
        }
    }
    sampler.abort();

    match any_err {
        Some(e) => {
            debug_log(&format!("parallel_download HATA: {}", e));
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
            debug_log("parallel_download TAMAM");
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
            debug_log(&format!("single URI hata: {}", e));
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
            debug_log(&format!("single HTTP {} ({} B)", r.status(), r.content_length().unwrap_or(0)));
            PROGRESS
                .lock()
                .unwrap()
                .get_mut(&link)
                .map(|s| s.error = Some(format!("HTTP {}", r.status())));
            return;
        }
        Err(e) => {
            debug_log(&format!("single HTTP hata: {}", e));
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
            debug_log(&format!("single file create hata: {}", e));
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
                    .map(|s| s.error = Some("İdare timeout: veri gelmedi".into()));
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
        "start" => "Cloudflare kontrol ediliyor...",
        "turnstile_clicked" => "Doğrulama kutusu tıklanıyor...",
        "turnstile_box_click" => "Doğrulama çözülüyor...",
        "btn_ready" => "İndirme butonu hazır, tıklanıyor...",
        "clicked" => "Buton tıklandı, bekleniyor...",
        "dlpass_ok" => "Oturum çerezi alındı, link çözülüyor...",
        "no_button" => "İndirme butonu bulunamadı!",
        "give_up" => "Çözüm başarısız oldu!",
        _ => "Link çözülüyor...",
    }
}

#[tauri::command]
async fn start_download(app: AppHandle, link: String, save_dir: String, parts: u64) -> Result<String, String> {
    debug_log(&format!("start_download: {} parts={}", link, parts));
    PROGRESS.lock().unwrap().insert(
        link.clone(),
        ProgState {
            progress: 0.0,
            downloaded: 0,
            total: 0,
            error: None,
            paused: false,
            sp: None,
            status: Some("Link çözülüyor (Cloudflare)...".into()),
        },
    );
    CTRL.lock().unwrap().insert(link.clone(), CtrlState::Running);

    let (dl_url, filename) = match resolve_download_url(&app, &link).await {
        Ok(v) => v,
        Err(e) => {
            debug_log(&format!("resolve HATA: {}", e));
            return Err(e);
        }
    };
    debug_log(&format!("resolve OK: {} -> {}", filename, dl_url));
    PROGRESS
        .lock()
        .unwrap()
        .get_mut(&link)
        .map(|s| s.status = Some("İndirme hazırlanıyor...".into()));
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
        debug_log(&format!("probe_total: {} B", total));
        let parts = parts.clamp(1, 16);
        if total == 0 || parts <= 1 {
            debug_log("single_download basliyor");
            single_download(link_c, dl_url, sp, total).await;
        } else {
            debug_log(&format!("parallel_download basliyor parts={}", parts));
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
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(
            std::env::temp_dir().join("ff_panic.txt"),
            format!("PANIC: {:?}", info),
        );
    }));
    let _ = std::fs::write(std::env::temp_dir().join("ff_debug.txt"), "startup ok\n");
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
