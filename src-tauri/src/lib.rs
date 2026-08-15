use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use http::HeaderValue;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
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

// Real, trusted OS-level mouse input (Windows SendInput path). Produces events
// that are indistinguishable from a human user, so it does NOT trip anti-bot.
#[cfg(target_os = "windows")]
mod real_mouse {
    use std::time::Duration;

    const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
    const MOUSEEVENTF_LEFTUP: u32 = 0x0004;

    #[link(name = "user32")]
    extern "system" {
        fn SetCursorPos(X: i32, Y: i32) -> i32;
        fn GetCursorPos(lpPoint: *mut POINT) -> i32;
        fn mouse_event(dwFlags: u32, dx: i32, dy: i32, dwData: u32, dwExtraInfo: usize);
    }

    #[repr(C)]
    struct POINT {
        x: i32,
        y: i32,
    }

    fn rng() -> u64 {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(12345)
    }

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (*seed >> 16 & 0x7fff) as f64 / 32768.0
    }

    // Move the cursor from wherever it is to (tx, ty) along a slightly curved,
    // jittered, ease-out path, pause like a human, then click once.
    pub fn human_click(tx: i32, ty: i32) {
        let mut seed = rng();

        let mut cur = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cur) };
        let (sx, sy) = (cur.x, cur.y);

        let steps = 16 + (lcg(&mut seed) * 10.0) as u32;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let ease = 1.0 - (1.0 - t) * (1.0 - t);
            let arc = (t * std::f64::consts::PI).sin() * (8.0 + 12.0 * lcg(&mut seed));
            let jx = (lcg(&mut seed) - 0.5) * 7.0;
            let jy = (lcg(&mut seed) - 0.5) * 7.0;
            let x = (sx as f64 + (tx as f64 - sx as f64) * ease + arc + jx) as i32;
            let y = (sy as f64 + (ty as f64 - sy as f64) * ease + arc + jy) as i32;
            unsafe { SetCursorPos(x, y) };
            std::thread::sleep(Duration::from_millis(12 + (lcg(&mut seed) * 9.0) as u64));
        }

        // Human pause before pressing.
        std::thread::sleep(Duration::from_millis(110 + (lcg(&mut seed) * 140.0) as u64));

        unsafe { mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0) };
        std::thread::sleep(Duration::from_millis(70 + (lcg(&mut seed) * 80.0) as u64));
        unsafe { mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0) };
    }
}


#[derive(Clone, Serialize)]
struct DownloadInfo {
    progress: f64,
    downloaded: u64,
    total: u64,
    error: Option<String>,
    paused: bool,
    status: Option<String>,
}

#[derive(Clone, Serialize)]
struct UpdateEntry {
    name: String,
    url: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct ContainerPart {
    name: String,
    url: String,
}

fn get_links_from_page(html: &str) -> Result<Vec<String>, String> {
    let document = Html::parse_document(html);

    let link_sel = Selector::parse("a[href*='fuckingfast.co']")
        .map_err(|e| format!("Selector: {}", e))?;

    let mut links: Vec<String> = Vec::new();
    for el in document.select(&link_sel) {
        if let Some(h) = el.value().attr("href") {
            let mut h = h.trim().to_string();
            if h.starts_with("//") {
                h = format!("https:{}", h);
            }
            if h.starts_with("http") {
                links.push(h);
            }
        }
    }

    if links.is_empty() {
        return Err("no fuckingfast link found".into());
    }

    // Prefer links inside spoiler content (multi-part downloads) when present.
    let spoiler_sel = Selector::parse("div.su-spoiler-content")
        .map_err(|e| format!("Selector: {}", e))?;
    let mut spoiler_links: Vec<String> = Vec::new();
    for sp in document.select(&spoiler_sel) {
        for el in sp.select(&link_sel) {
            if let Some(h) = el.value().attr("href") {
                let mut h = h.trim().to_string();
                if h.starts_with("//") {
                    h = format!("https:{}", h);
                }
                if h.starts_with("http") {
                    spoiler_links.push(h);
                }
            }
        }
    }

    let mut results = if !spoiler_links.is_empty() {
        spoiler_links
    } else {
        links
    };

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



fn sanitize_label(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '/' || c == ':' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

const RESOLVER_JS: &str = r#"(function () {
  if (window.__ff_auto_click_active) return;
  window.__ff_auto_click_active = true;

  // Block popup windows / ads
  try { window.open = () => null; } catch(e) {}

  const findBtn = () =>
    document.querySelector('a.gay-button, button.gay-button, [class*="gay-button"]');

  // Direct HTMX POST to /go endpoint with real token
  const doPost = (el) => {
    const hxPost = el && el.getAttribute && el.getAttribute('hx-post');
    if (!hxPost) return;
    const token = window.turnstileToken || '';
    try {
      if (window.htmx) {
        window.htmx.ajax('POST', hxPost, {
          source: el,
          swap: 'none',
          values: { 'cf-turnstile-response': token }
        });
      }
    } catch(e) {}
    // Also try direct fetch as fallback
    try {
      fetch('https://fuckingfast.co' + hxPost, {
        method: 'POST',
        credentials: 'include',
        headers: {
          'HX-Request': 'true',
          'Content-Type': 'application/x-www-form-urlencoded',
          'HX-Current-URL': location.href,
        },
        body: 'cf-turnstile-response=' + encodeURIComponent(token)
      }).then(r => {
        const redir = r.headers.get('HX-Redirect') || r.headers.get('hx-redirect');
        if (redir) {
          const url = redir.startsWith('/') ? 'https://fuckingfast.co' + redir : redir;
          location.href = url;
        }
      });
    } catch(e) {}
  };

  const clickBtn = (el) => {
    if (!el) return;
    // Ensure visual/HTMX condition is met
    try { window.dlCleared = true; window.turnstileSuccess = true; } catch(e) {}
    try { el.style.opacity = '1'; el.style.cursor = 'pointer'; } catch(e) {}
    try { if (el.__x) { el.__x.$data.turnstileSuccess = true; el.__x.$data.cleared = true; } } catch(e) {}
    // Fire all click events
    try { el.focus(); el.click(); } catch(e) {}
    try {
      ['mousedown', 'mouseup', 'click'].forEach(t =>
        el.dispatchEvent(new MouseEvent(t, { bubbles: true, cancelable: true }))
      );
    } catch(e) {}
    try { if (window.htmx) window.htmx.trigger(el, 'click'); } catch(e) {}
    // Direct POST as backup
    doPost(el);
  };

  let stage = 0;

  const tryDownload = () => {
    if (stage > 0) return;
    const btn = findBtn();
    if (!btn) return;

    // Check real token or button unlocked (opacity high = Turnstile passed)
    const hasToken = !!window.turnstileToken || !!window.dlCleared;
    let opacity = 1;
    try { opacity = parseFloat(window.getComputedStyle(btn).opacity) || 1; } catch(e) {}
    const btnUnlocked = opacity > 0.8;

    if (!hasToken && !btnUnlocked) return; // Still waiting for Turnstile

    stage = 1;
    console.log('[FF] Token ready / button unlocked, clicking...');

    // Click 1 then Click 2 with 800ms gap
    setTimeout(() => {
      clickBtn(findBtn() || btn);
      setTimeout(() => {
        clickBtn(findBtn() || btn);
        stage = 2;
      }, 800);
    }, 1000);
  };

  // Hook Turnstile callback so we click the moment the real token arrives
  try {
    const origCallback = window.onTurnstileSuccess;
    window.onTurnstileSuccess = (token) => {
      window.turnstileToken = token;
      if (origCallback) origCallback(token);
      setTimeout(tryDownload, 300); // small delay for Alpine to update
    };
    // Also patch turnstile render options if they exist
    const origRender = window.turnstile && window.turnstile.render;
    if (origRender) {
      window.turnstile.render = (container, params) => {
        const origCb = params && params.callback;
        if (params) {
          params.callback = (token) => {
            window.turnstileToken = token;
            if (origCb) origCb(token);
            setTimeout(tryDownload, 300);
          };
        }
        return origRender.call(window.turnstile, container, params);
      };
    }
  } catch(e) {}

  // Polling fallback every 500ms
  setInterval(tryDownload, 500);
})();
"#;

// Runs before the container page loads: block popups and strip known ad
// iframes/scripts. The filecrypt captcha (needed for the security check)
// lives on cutcaptcha.net / captcha.filecrypt.cc and is NOT blocked.
const CONTAINER_JS: &str = r#"
(function () {
  try { window.open = function () { return null; }; } catch (e) {}
  // Minimal view: hide everything except the captcha box and center it.
  try {
    var st = document.createElement('style');
    st.textContent =
      'html,body{height:100%;margin:0;overflow:hidden!important;background:#141414!important;}'
      + 'body *{visibility:hidden!important;}'
      + '.pow-captcha,.pow-captcha *{visibility:visible!important;}'
      + '.pow-captcha{position:fixed!important;top:50%!important;left:50%!important;transform:translate(-50%,-50%)!important;z-index:99999!important;}';
    document.head.appendChild(st);
  } catch (e) {}
  var BLOCK = [
    'meritvolleyballturban.com',
    'linkonclick.com',
    'jump/next.php',
    'adsterra.com',
    'propellerads.com',
    'popads.net'
  ];
  function isBad(el) {
    var src = (el && (el.src || el.href || el.getAttribute && (el.getAttribute('src') || el.getAttribute('href')))) || '';
    for (var i = 0; i < BLOCK.length; i++) {
      if (src.indexOf(BLOCK[i]) !== -1) return true;
    }
    return false;
  }
  function strip() {
    try {
      var els = document.querySelectorAll('iframe, script');
      for (var i = 0; i < els.length; i++) {
        if (isBad(els[i])) {
          try { if (els[i].parentNode) els[i].parentNode.removeChild(els[i]); } catch (e) {}
        }
      }
    } catch (e) {}
  }
  try {
    var mo = new MutationObserver(strip);
    mo.observe(document.documentElement, { childList: true, subtree: true });
  } catch (e) {}
  setInterval(strip, 1500);
})();
"#;

// Reads the decrypted file list directly from the DOM (no clicking needed),
// keeping ONLY the fuckingfast.co mirrors (each file appears once per host,
// e.g. datanodes.to + fuckingfast.co; we skip the others). Returns an object
// { found, parts: [{name, url}] }; evaluated in-page so the serialized JSON
// string arrives at the Rust callback.
const EXTRACT_PARTS_JS: &str = r#"
(function () {
  try {
    var rows = document.querySelectorAll('tr.kwj3');
    var parts = [];
    for (var i = 0; i < rows.length; i++) {
      var tr = rows[i];
      var hostA = tr.querySelector('a.external_link');
      if (!hostA) continue;
      var host = (hostA.getAttribute('href') || '').toLowerCase();
      if (host.indexOf('fuckingfast.co') === -1) continue;
      var td = tr.querySelector('td[title]');
      var name = td ? (td.getAttribute('title') || '') : '';
      var a = tr.querySelector('a.button.download[href*="/Link/"]');
      if (!a) continue;
      var url = a.getAttribute('href') || '';
      if (url.indexOf('http') !== 0) url = 'https://filecrypt.cc' + url;
      parts.push({ name: name, url: url });
    }
    return { found: rows.length > 0, parts: parts };
  } catch (e) {
    return { found: false, parts: [] };
  }
})()
"#;

// Reports the on-screen (CSS-pixel) center of the captcha box and its state so
// the Rust side can send a real, trusted mouse click at exactly that spot.
const CAPTCHA_POS_JS: &str = r#"
(function () {
  try {
    var c = document.querySelector('.pow-captcha');
    var box = document.querySelector('.pow-captcha__box');
    if (!c || !box) return { state: 'absent', cx: 0, cy: 0 };
    var r = box.getBoundingClientRect();
    return {
      state: (c.getAttribute('data-state') || '').toLowerCase(),
      cx: Math.round(r.left + r.width / 2),
      cy: Math.round(r.top + r.height / 2)
    };
  } catch (e) {
    return { state: 'absent', cx: 0, cy: 0 };
  }
})()
"#;


async fn wait_resolver(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
    window: &tauri::WebviewWindow,
    link: &str,
) -> Result<String, String> {
    let t0 = tokio::time::Instant::now();
    let mut last_inject = tokio::time::Instant::now();
    let mut has_centered = false;
    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(-10000, -10000)));
    let _ = window.show();
    let _ = window.eval(RESOLVER_JS);
    loop {
        let elapsed = t0.elapsed();
        if elapsed >= Duration::from_secs(360) {
            return Err("Timeout: could not solve Cloudflare".into());
        }
        tokio::select! {
            v = rx.recv() => match v {
                Some(d) => return Ok(d),
                None => return Err("resolver closed".to_string()),
            },
            _ = tokio::time::sleep(Duration::from_millis(400)) => {
                if !has_centered && elapsed >= Duration::from_secs(7) {
                    has_centered = true;
                    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(500, 600)));
                    let _ = window.center();
                    let _ = window.set_focus();
                }
                if last_inject.elapsed() >= Duration::from_secs(2) {
                    last_inject = tokio::time::Instant::now();
                    let _ = window.eval(RESOLVER_JS);
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
        sanitize_label(url.path().trim_start_matches('/')),
        stamp
    );

    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.close();
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .visible(false)
        .title("Resolving Download Link...")
        .decorations(false)
        .skip_taskbar(true)
        .inner_size(800.0, 800.0)
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

async fn try_direct_resolve(link: &str) -> Result<String, String> {
    let base_link = link.split('#').next().unwrap_or(link);
    let file_id = base_link
        .split('/')
        .filter(|s| !s.is_empty())
        .last()
        .ok_or("no file id")?;

    let go_url = format!("https://fuckingfast.co/f/{}/go", file_id);
    let uri: http::Uri = go_url.parse().map_err(|e| format!("URI: {}", e))?;

    let mut req = wreq::Request::new(Method::POST, uri);
    req.headers_mut().insert("HX-Request", HeaderValue::from_static("true"));
    req.headers_mut().insert(
        "HX-Current-URL",
        HeaderValue::try_from(format!("https://fuckingfast.co/f/{}", file_id))
            .map_err(|e| format!("Header: {}", e))?,
    );
    req.headers_mut().insert("HX-Target", HeaderValue::from_static("container"));
    req.headers_mut().insert("Origin", HeaderValue::from_static("https://fuckingfast.co"));
    req.headers_mut().insert(
        "Referer",
        HeaderValue::try_from(format!("https://fuckingfast.co/f/{}", file_id))
            .map_err(|e| format!("Header: {}", e))?,
    );
    req.headers_mut().insert("Content-Type", HeaderValue::from_static("application/x-www-form-urlencoded"));

    let resp = HTTP_CLIENT.execute(req).await.map_err(|e| format!("HTTP: {}", e))?;

    if resp.status() == 429 || resp.status() == 403 {
        return Err("Cloudflare/RateLimited".into());
    }

    if let Some(hx) = resp.headers().get("HX-Redirect").or_else(|| resp.headers().get("hx-redirect")) {
        if let Ok(s) = hx.to_str() {
            let url = if s.starts_with('/') {
                format!("https://fuckingfast.co{}", s)
            } else {
                s.to_string()
            };
            return Ok(url);
        }
    }

    let text = resp.text().await.map_err(|e| format!("Body: {}", e))?;
    if let Some(m) = text.find("https://dl.fuckingfast.co/") {
        let rest = &text[m..];
        let end = rest.find(&['"', '\'', ' ', '<', '>', '\r', '\n'][..]).unwrap_or(rest.len());
        return Ok(rest[..end].to_string());
    }

    Err("No redirect header or dl url in body".into())
}

async fn resolve_download_url(app: &AppHandle, link: &str) -> Result<(String, String), String> {
    let filename = link
        .split('#')
        .nth(1)
        .filter(|f| !f.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            link.rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or(link)
                .to_string()
        });

    // 1. Try direct HTTP POST resolution first (instant if session cookies exist)
    if let Ok(dl_url) = try_direct_resolve(link).await {
        return Ok((dl_url, filename));
    }

    // 2. Otherwise open Webview window to solve Cloudflare & acquire cookie
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

fn parse_updates_from_page(html: &str) -> Vec<UpdateEntry> {
    let document = Html::parse_document(html);
    let Some(box_sel) = Selector::parse("div[style*='9aff612e']").ok() else {
        return Vec::new();
    };
    let Some(link_sel) = Selector::parse("a[href*='filecrypt.cc/Container/']").ok() else {
        return Vec::new();
    };

    let mut entries: Vec<UpdateEntry> = Vec::new();
    for div in document.select(&box_sel) {
        for a in div.select(&link_sel) {
            let Some(url) = a.value().attr("href") else { continue };
            let url = url.trim().to_string();
            let name = a.text().collect::<String>().trim().to_string();
            if url.is_empty() || name.is_empty() {
                continue;
            }
            if entries.iter().any(|e| e.url == url) {
                continue;
            }
            entries.push(UpdateEntry { name, url });
        }
    }
    entries
}

#[tauri::command]
async fn get_updates(url: String) -> Result<Vec<UpdateEntry>, String> {
    let html = fetch_page(&url).await?;
    Ok(parse_updates_from_page(&html))
}

#[tauri::command]
async fn open_container(
    app: AppHandle,
    url: String,
    captcha_mode: String,
) -> Result<Vec<ContainerPart>, String> {
    let parsed = Url::parse(&url).map_err(|e| format!("URL parse: {}", e))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let label = format!("ff_fc_{}", stamp);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<ContainerPart>>();

    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title("FileCrypt Container - solve the security check, links are read automatically")
        .inner_size(560.0, 480.0)
        .initialization_script(CONTAINER_JS)
        .build()
        .map_err(|e| format!("container window: {}", e))?;

    let _ = window.show();
    let _ = window.set_focus();

    let t0 = tokio::time::Instant::now();
    // Only ever click the captcha once, and only after it has had time to load
    // and settle into its interactive "idle" state.
    let clicked = Arc::new(AtomicBool::new(false));
    loop {
        // Timeout so the command never hangs forever.
        if t0.elapsed() >= Duration::from_secs(600) {
            let _ = window.close();
            return Err("Timeout waiting for container parts".into());
        }

        // If the user closed the window before parts were found, give up.
        if app.get_webview_window(&label).is_none() {
            let _ = window.close();
            return Ok(Vec::new());
        }

        // Poll the DOM: once the decrypted list is present it's read directly.
        if let Some(win) = app.get_webview_window(&label) {
            let tx = tx.clone();
            let _ = win.eval_with_callback(EXTRACT_PARTS_JS, move |res| {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&res) {
                    if v.get("found").and_then(|f| f.as_bool()) == Some(true) {
                        let parts: Vec<ContainerPart> = v
                            .get("parts")
                            .and_then(|p| serde_json::from_value(p.clone()).ok())
                            .unwrap_or_default();
                        let _ = tx.send(parts);
                    }
                }
            });

            // Real, trusted mouse click on the captcha once it is idle and has
            // had time to settle. Only used in "auto" mode; in "manual" mode the
            // user clicks the box themselves. Coordinates are CSS pixels ->
            // physical screen pixels via the window position + scale factor.
            let auto = captcha_mode.eq_ignore_ascii_case("auto");
            if auto && !clicked.load(Ordering::Relaxed) && t0.elapsed() >= Duration::from_secs(6) {
                let clicked = clicked.clone();
                let app2 = app.clone();
                let label2 = label.clone();
                let _ = win.eval_with_callback(CAPTCHA_POS_JS, move |res| {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&res) {
                        let state = v.get("state").and_then(|s| s.as_str()).unwrap_or("");
                        let cx = v.get("cx").and_then(|c| c.as_f64()).unwrap_or(0.0);
                        let cy = v.get("cy").and_then(|c| c.as_f64()).unwrap_or(0.0);
                        if state == "idle" && cx > 0.0 && cy > 0.0 {
                            // Convert CSS position to a trusted physical click.
                            if let Some(win2) = app2.get_webview_window(&label2) {
                                if let (Ok(pos), Ok(scale)) =
                                    (win2.inner_position(), win2.scale_factor())
                                {
                                    let sx = pos.x + (cx * scale) as i32;
                                    let sy = pos.y + (cy * scale) as i32;
                                    if clicked.compare_exchange(
                                        false,
                                        true,
                                        Ordering::Relaxed,
                                        Ordering::Relaxed,
                                    ) == Ok(false)
                                    {
                                        std::thread::spawn(move || {
                                            real_mouse::human_click(sx, sy);
                                        });
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }

        tokio::select! {
            p = rx.recv() => {
                if let Some(parts) = p {
                    let _ = window.close();
                    return Ok(parts);
                }
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(700)) => {}
        }
    }

    let _ = window.close();
    Ok(Vec::new())
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

    let (dl_url, filename) = {
        let host = link.split('/').nth(2).unwrap_or("").to_lowercase();
        if host.contains("dl.fuckingfast.co") {
            let filename = link
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or("download")
                .to_string();
            (link.clone(), filename)
        } else {
            resolve_download_url(&app, &link).await?
        }
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
            get_updates,
            open_container,
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

#[cfg(test)]
mod tests {
    use super::*;

    const WD2_SPOILER: &str = r#"<div class="su-spoiler-content su-u-clearfix su-u-trim">
<a href="https://fuckingfast.co/g1pdp1kuolm5#Watch_Dogs_2_--_fitgirl-repacks.site_--_.part01.rar" target="_blank" rel="noopener nofollow">part01.rar</a>
<a href="https://fuckingfast.co/w7m35s4qeg8h#Watch_Dogs_2_--_fitgirl-repacks.site_--_.part02.rar" target="_blank" rel="noopener nofollow">part02.rar</a>
<a href="https://fuckingfast.co/a5do7kypp5d6#fg-optional-bonus-content.bin" target="_blank" rel="noopener nofollow">bonus.bin</a>
<a href="https://fuckingfast.co/86skntewlrun#fg-selective-brazilian.bin.part1.rar" target="_blank" rel="noopener nofollow">brazilian part1</a>
</div>"#;

    #[test]
    fn extracts_fuckingfast_links_from_spoiler_div() {
        let links = get_links_from_page(WD2_SPOILER).unwrap();
        assert_eq!(links.len(), 4);
        assert!(links[0].ends_with("part01.rar"));
        assert!(links[1].ends_with("part02.rar"));
        assert!(links[2].starts_with("https://fuckingfast.co/"));
    }

    #[test]
    fn extracts_single_link_without_spoiler() {
        let html = r#"<div class="entry-content"><a href="https://fuckingfast.co/abc123#game.rar">Filehoster: FuckingFast</a></div>"#;
        let links = get_links_from_page(html).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "https://fuckingfast.co/abc123#game.rar");
    }

    #[test]
    fn errors_when_no_fuckingfast_links() {
        let html = "<html><body><p>nothing here</p></body></html>";
        assert!(get_links_from_page(html).is_err());
    }

    #[test]
    fn extracts_updates_from_green_div() {
        let html = r#"<h3>Game Updates &#8211; Direct Links only</h3>
<div style="background-color: #9aff612e; border: 1px solid #159311; border-radius: 10px; padding:20px; margin-bottom: 20px">
<ol>
<li><a href="https://filecrypt.cc/Container/DB6F829416.html">TEKKEN.8.Update.v2.08.00.incl.DLC-RUNE (3 parts)</a> (Source: scene)<br />
or<br />
<a href="https://filecrypt.cc/Container/68C5B68CB1.html">TEKKEN_8_Update_from_v2.06.01_to_v2.08.00-ElAmigos (2 parts)</a></p>
<li><a href="https://filecrypt.cc/Container/6185D28BB9.html">TEKKEN.8.Update.v2.09.00.incl.DLC-RUNE (2 parts)</a>
</ol>
</div>"#;
        let entries = parse_updates_from_page(html);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "TEKKEN.8.Update.v2.08.00.incl.DLC-RUNE (3 parts)");
        assert_eq!(entries[0].url, "https://filecrypt.cc/Container/DB6F829416.html");
        assert!(entries.iter().all(|e| e.url.contains("filecrypt.cc/Container/")));
    }

    #[test]
    fn updates_ignores_non_green_links() {
        let html = r#"<div class="entry-content">
<a href="https://filecrypt.cc/Container/DB6F829416.html">bad</a>
<a href="https://fuckingfast.co/abc#x.rar">download</a>
</div>"#;
        let entries = parse_updates_from_page(html);
        assert!(entries.is_empty());
    }
}