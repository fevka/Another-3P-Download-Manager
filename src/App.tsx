import { useState, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";

function extractFileName(link: string): string {
  const afterHash = link.split("#").pop();
  if (afterHash && afterHash.length > 0) return decodeURIComponent(afterHash);
  try {
    const u = new URL(link);
    const segs = u.pathname.split("/").filter(Boolean);
    if (segs.length > 0) return decodeURIComponent(segs[segs.length - 1]);
  } catch {}
  return link;
}

function naturalCompare(a: string, b: string): number {
  const re = /(\d+)|(\D+)/g;
  const aParts = a.match(re) || [];
  const bParts = b.match(re) || [];
  const len = Math.min(aParts.length, bParts.length);
  for (let i = 0; i < len; i++) {
    const an = parseInt(aParts[i], 10);
    const bn = parseInt(bParts[i], 10);
    if (String(an) === aParts[i] && String(bn) === bParts[i]) {
      if (an !== bn) return an - bn;
    } else {
      const cmp = aParts[i].localeCompare(bParts[i]);
      if (cmp !== 0) return cmp;
    }
  }
  return aParts.length - bParts.length;
}

function formatSpeed(bytesPerSec: number): string {
  if (bytesPerSec >= 1024 * 1024) return `${(bytesPerSec / 1024 / 1024).toFixed(1)} MB/s`;
  if (bytesPerSec >= 1024) return `${(bytesPerSec / 1024).toFixed(0)} KB/s`;
  return `${bytesPerSec.toFixed(0)} B/s`;
}

interface DownloadItem {
  link: string;
  file_name: string;
  status: "pending" | "resolving" | "downloading" | "paused" | "done" | "error";
  progress: number;
  downloaded?: number;
  totalBytes?: number;
  error?: string;
  speed?: number;
  checked: boolean;
  resolverMsg?: string;
}

function App() {
  const [inputText, setInputText] = useState("");
  const [downloadDir, setDownloadDir] = useState(() => localStorage.getItem("downloadDir") || "");
  const [items, setItems] = useState<DownloadItem[]>([]);
  const [isProcessing, setIsProcessing] = useState(false);
  const [maxConcurrent, setMaxConcurrent] = useState(1);
  const [connections, setConnections] = useState(4);
  const [modalMsg, setModalMsg] = useState<string | null>(null);
  const cancelRef = useRef(false);
  const speedRef = useRef(new Map<string, { downloaded: number; time: number; ema: number }>());

  const parseLinks = useCallback(() => {
    const lines = inputText
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.length > 0);

    const newItems: DownloadItem[] = lines
      .map((link) => ({
        link,
        file_name: "",
        status: "pending" as const,
        progress: 0,
        checked: false,
      }))
      .sort((a, b) => naturalCompare(extractFileName(a.link), extractFileName(b.link)));
    setItems(newItems);
  }, [inputText]);

  const pickDir = useCallback(async () => {
    const dir = await open({ directory: true, multiple: false });
    if (dir) {
      setDownloadDir(dir);
      localStorage.setItem("downloadDir", dir);
    }
  }, []);

  const getLinksFn = useCallback(async () => {
    try {
      setIsProcessing(true);
      const url = inputText.trim();
      if (!url) return;

      const links = await invoke<string[]>("get_links", { url });
      setInputText(links.join("\n"));
      const newItems: DownloadItem[] = links
        .map((link) => ({
          link,
          file_name: "",
          status: "pending" as const,
          progress: 0,
          checked: false,
        }))
        .sort((a, b) => naturalCompare(extractFileName(a.link), extractFileName(b.link)));
      setItems(newItems);
    } catch (e) {
      setModalMsg(`Error: ${e}`);
    } finally {
      setIsProcessing(false);
    }
  }, [inputText]);

  const toggleCheck = useCallback((idx: number) => {
    setItems((prev) =>
      prev.map((it, i) => (i === idx ? { ...it, checked: !it.checked } : it))
    );
  }, []);

  const selectAll = useCallback(() => {
    setItems((prev) =>
      prev.map((it) => ({
        ...it,
        checked: it.status === "pending" || it.status === "error",
      }))
    );
  }, []);

  const deselectOptional = useCallback(() => {
    setItems((prev) =>
      prev.map((it) => {
        const name = (it.file_name || extractFileName(it.link)).toLowerCase();
        return name.includes("optional") ? { ...it, checked: false } : it;
      })
    );
  }, []);

  const deselectAll = useCallback(() => {
    setItems((prev) => prev.map((it) => ({ ...it, checked: false })));
  }, []);

  const setItemStatus = useCallback(
    (idx: number, patch: Partial<DownloadItem>) => {
      setItems((prev) =>
        prev.map((it, i) => (i === idx ? { ...it, ...patch } : it))
      );
    },
    []
  );

  const pollProgress = async (link: string, idx: number, startPromise: Promise<string>) => {
    let fileName: string | null = null;
    let resolveDone = false;
    let resolveErr: unknown = null;
    startPromise.then(
      (n) => {
        fileName = n;
        resolveDone = true;
      },
      (e) => {
        resolveErr = e;
        resolveDone = true;
      }
    );

    while (true) {
      await new Promise((r) => setTimeout(r, 500));

      if (!resolveDone) {
        const info = await invoke<{
          progress: number;
          downloaded: number;
          total: number;
          error: string | null;
          paused: boolean;
          status: string | null;
        }>("get_download_progress", { link });
        if (info.status) {
          setItemStatus(idx, { status: "resolving", resolverMsg: info.status });
        }
        continue;
      }

      if (resolveErr) {
        if (cancelRef.current) {
          setItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined, resolverMsg: undefined });
        } else {
          setItemStatus(idx, { status: "error", error: String(resolveErr), resolverMsg: undefined });
        }
        return;
      }

      const info = await invoke<{
        progress: number;
        downloaded: number;
        total: number;
        error: string | null;
        paused: boolean;
        status: string | null;
      }>("get_download_progress", { link });

      if (info.error) {
        speedRef.current.delete(link);
        if (info.error === "Cancelled") {
          setItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined, speed: 0, resolverMsg: undefined });
        } else {
          setItemStatus(idx, { status: "error", error: info.error!, progress: 0, downloaded: 0, speed: 0, resolverMsg: undefined });
        }
        return;
      }

      if (info.paused) {
        setItemStatus(idx, { status: "paused", speed: 0 });
      }

      if (info.downloaded > 0) {
        const now = Date.now();
        const prev = speedRef.current.get(link);
        let speed = 0;
        if (prev) {
          const dt = (now - prev.time) / 1000;
          if (dt > 0) speed = (info.downloaded - prev.downloaded) / dt;
        }
        const ema =
          prev && prev.ema > 0
            ? prev.ema * 0.6 + Math.max(0, speed) * 0.4
            : Math.max(0, speed);
        speedRef.current.set(link, { downloaded: info.downloaded, time: now, ema });
        setItemStatus(idx, {
          status: info.paused ? "paused" : "downloading",
          file_name: fileName ?? undefined,
          progress: info.progress,
          downloaded: info.downloaded,
          totalBytes: info.total,
          speed: info.paused ? 0 : ema,
          resolverMsg: undefined,
        });
      }

      if (info.progress >= 100) {
        speedRef.current.delete(link);
        break;
      }
    }
  };

  const downloadSingle = useCallback(
    async (link: string, idx: number) => {
      if (!downloadDir || cancelRef.current) return;

      setItemStatus(idx, { status: "resolving" });

      try {
        const startPromise = invoke<string>("start_download", {
          link,
          saveDir: downloadDir,
          parts: connections,
        });

        await pollProgress(link, idx, startPromise);

        setItems((prev) => {
          const item = prev[idx];
          if (item && (item.status === "downloading" || item.status === "paused")) {
            return prev.map((it, i) =>
              i === idx ? { ...it, status: "done", progress: 100, error: undefined } : it
            );
          }
          return prev;
        });
      } catch (e) {
        if (cancelRef.current) {
          setItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined, resolverMsg: undefined });
        } else {
          setItemStatus(idx, { status: "error", error: String(e), resolverMsg: undefined });
        }
      } finally {
        invoke("clear_download", { link }).catch(() => {});
      }
    },
    [downloadDir, connections, setItemStatus]
  );

  const startDownloads = useCallback(async () => {
    if (!downloadDir) {
      setModalMsg("Please select a download directory first");
      return;
    }
    const toDownload = items.filter((it) => it.checked && it.status === "pending");
    if (toDownload.length === 0) return;

    cancelRef.current = false;
    setIsProcessing(true);
    let nextIndex = 0;

    const worker = async () => {
      while (!cancelRef.current) {
        const idx = nextIndex++;
        if (idx >= items.length) break;
        const item = items[idx];
        if (!item.checked || item.status !== "pending") continue;
        await downloadSingle(item.link, idx);
      }
    };

    const workers = Array.from({ length: maxConcurrent }, () => worker());
    await Promise.all(workers);

    if (!cancelRef.current) {
      setItems((prev) => prev.map((it) => ({ ...it, checked: false })));
    }
    setIsProcessing(false);
  }, [items, downloadDir, maxConcurrent, downloadSingle]);

  const cancelAll = useCallback(async () => {
    cancelRef.current = true;
    const active = items.filter(
      (it) =>
        it.status === "downloading" ||
        it.status === "paused" ||
        it.status === "resolving"
    );
    for (const item of active) {
      await invoke("cancel_download", { link: item.link }).catch(() => {});
    }
    setIsProcessing(false);
  }, [items]);

  const pauseItem = useCallback(async (link: string) => {
    await invoke("pause_download", { link });
  }, []);

  const resumeItem = useCallback(
    async (link: string, idx: number) => {
      await invoke("resume_download", { link });
      setItemStatus(idx, { status: "downloading" });
    },
    [setItemStatus]
  );

  const cancelItem = useCallback(
    async (link: string, idx: number) => {
      await invoke("cancel_download", { link });
      setItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined });
    },
    [setItemStatus]
  );

  const anyActive = items.some(
    (it) => it.status === "downloading" || it.status === "paused" || it.status === "resolving"
  );

  const checkedCount = items.filter((it) => it.checked).length;
  const canDownload = checkedCount > 0 && !anyActive;
  const optionalChecked = items.some((it) => {
    if (!it.checked) return false;
    return (it.file_name || extractFileName(it.link)).toLowerCase().includes("optional");
  });

  const statusIcon = (status: string) => {
    switch (status) {
      case "resolving": return "🔎";
      case "downloading": return "⬇";
      case "paused": return "⏸";
      case "done": return "★";
      case "error": return "✖";
      default: return "";
    }
  };

  return (
    <div className="container">
      <div className="titlebar" data-tauri-drag-region>
        <div className="titlebar-btns">
          <button className="tb-btn" onClick={() => invoke("window_minimize")}>─</button>
          <button className="tb-btn" onClick={() => invoke("window_toggle_maximize")}>□</button>
          <button className="tb-btn tb-close" onClick={() => invoke("window_close")}>✕</button>
        </div>
      </div>
      <header data-tauri-drag-region>
        <h1>FF Downloader</h1>
      </header>

      <section className="input-section">
        <textarea
          placeholder="PASTE URL OR LINKS (ONE PER LINE)..."
          value={inputText}
          onChange={(e) => setInputText(e.target.value)}
          rows={6}
        />
        <div className="btn-row">
          <button onClick={getLinksFn} disabled={isProcessing}>FETCH URL</button>
          <button onClick={parseLinks} disabled={isProcessing}>PARSE</button>
          <div className="concurrency-group">
            <label>Simultaneous:</label>
            <select
              value={maxConcurrent}
              onChange={(e) => setMaxConcurrent(Number(e.target.value))}
              disabled={isProcessing}
            >
              <option value={1}>1</option>
              <option value={2}>2</option>
              <option value={3}>3</option>
              <option value={4}>4</option>
              <option value={5}>5</option>
            </select>
          </div>
          <div className="concurrency-group">
            <label>Conn:</label>
            <select
              value={connections}
              onChange={(e) => setConnections(Number(e.target.value))}
              disabled={isProcessing}
            >
              <option value={1}>1</option>
              <option value={2}>2</option>
              <option value={3}>3</option>
              <option value={4}>4</option>
              <option value={6}>6</option>
              <option value={8}>8</option>
            </select>
          </div>
        </div>
      </section>

      <section className="dir-section">
        <input type="text" placeholder="SELECT SAVE PATH..." value={downloadDir} readOnly />
        <button onClick={pickDir}>BROWSE</button>
      </section>

      {items.length > 0 && (
        <section className="items-section">
          <div className="items-header">
            <span>{items.length} file(s)</span>
            <div className="items-header-btns">
              {!anyActive && (
                <>
                  <button onClick={selectAll} disabled={isProcessing} className="sel-btn">SELECT ALL</button>
                  <button onClick={deselectOptional} disabled={isProcessing || !optionalChecked} className="sel-btn">DESELECT OPTIONAL</button>
                  <button onClick={deselectAll} disabled={isProcessing || checkedCount === 0} className="sel-btn">DESELECT ALL</button>
                </>
              )}
              <button
                onClick={anyActive ? cancelAll : startDownloads}
                disabled={!anyActive && !canDownload}
                className={anyActive ? "cancel-btn" : "start-btn"}
              >
                {anyActive ? "CANCEL DOWNLOADS" : "DOWNLOAD"}
              </button>
            </div>
          </div>
          <div className="items-list">
            {items.map((item, idx) => (
              <div key={idx} className={`item ${item.status}`}>
                <div className="item-info">
                  <input
                    type="checkbox"
                    className="item-check"
                    checked={item.checked}
                    onChange={() => toggleCheck(idx)}
                    disabled={item.status !== "pending" && item.status !== "error"}
                  />
                  {statusIcon(item.status) && <span className="item-status">{statusIcon(item.status)}</span>}
                  <span className="item-name">
                    {item.file_name || extractFileName(item.link)}
                  </span>
                  <div className="item-actions">
                    {item.status === "pending" && !anyActive && (
                      <button
                        className="item-dl-btn"
                        onClick={async () => {
                          if (!downloadDir) { setModalMsg("Please select a download directory first"); return; }
                          cancelRef.current = false;
                          setIsProcessing(true);
                          await downloadSingle(item.link, idx);
                          setIsProcessing(false);
                        }}
                      >
                        DOWNLOAD
                      </button>
                    )}
                    {item.status === "downloading" && (
                      <>
                        <button className="item-pause-btn" onClick={() => pauseItem(item.link)}>⏸</button>
                        <button className="item-cancel-btn" onClick={() => cancelItem(item.link, idx)}>✕</button>
                      </>
                    )}
                    {item.status === "paused" && (
                      <>
                        <button className="item-resume-btn" onClick={() => resumeItem(item.link, idx)}>▶</button>
                        <button className="item-cancel-btn" onClick={() => cancelItem(item.link, idx)}>✕</button>
                      </>
                    )}
                  </div>
                </div>
                <div className="item-progress">
                  {(item.status === "downloading" || item.status === "paused") && (
                    <div className="progress-bar">
                      <div className="progress-fill" style={{ width: `${item.progress}%` }} />
                    </div>
                  )}
                  <span className="progress-text">
                    {item.status === "downloading" || item.status === "paused"
                      ? `${item.totalBytes && item.totalBytes > 0
                          ? `${item.progress.toFixed(1)}% (${((item.downloaded || 0) / 1024 / 1024).toFixed(0)} MB / ${(item.totalBytes / 1024 / 1024).toFixed(0)} MB)`
                          : `${((item.downloaded || 0) / 1024 / 1024).toFixed(0)} MB`}${item.speed ? ` @ ${formatSpeed(item.speed)}` : ""}`
                      : item.status === "done"
                        ? (item.totalBytes && item.totalBytes > 0 ? "100%" : `${((item.downloaded || 0) / 1024 / 1024).toFixed(0)} MB`)
                        : item.status === "error"
                          ? "Error"
                          : ""}
                  </span>
                </div>
                {item.error && <div className="item-error">{item.error}</div>}
                {item.resolverMsg && item.status === "resolving" && (
                  <div className="item-resolver">{item.resolverMsg}</div>
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      {modalMsg && (
        <div className="modal-overlay" onClick={() => setModalMsg(null)}>
          <div className="modal-box">
            <div className="modal-icon">⚠</div>
            <div className="modal-text">{modalMsg}</div>
            <button className="modal-btn" onClick={() => setModalMsg(null)}>OK</button>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
