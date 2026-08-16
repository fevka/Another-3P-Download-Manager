import { useState, useCallback, useRef, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { LANGS, getDict, LangCode } from "./i18n";
import "./App.css";

export type ThemeName = "cyberpunk" | "gold" | "matrix" | "retro" | "fp";

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
  link: string;  file_name: string;
  status: "pending" | "resolving" | "downloading" | "paused" | "done" | "error";
  progress: number;
  downloaded?: number;
  totalBytes?: number;
  error?: string;
  speed?: number;
  checked: boolean;
  resolverMsg?: string;
}

interface ContainerPart {
  name: string;
  url: string;
}

function App() {
  const [inputText, setInputText] = useState("");
  const [pageUrl, setPageUrl] = useState("");
  const [lang, setLang] = useState<LangCode>(() => (localStorage.getItem("lang") as LangCode) || "en");
  const [theme, setTheme] = useState<ThemeName>(() => (localStorage.getItem("theme") as ThemeName) || "retro");
  const [captchaMode, setCaptchaMode] = useState<"auto" | "manual">(() => (localStorage.getItem("captchaMode") as "auto" | "manual") || "manual");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const t = getDict(lang);
  const [downloadDir, setDownloadDir] = useState(() => localStorage.getItem("downloadDir") || "");
  const [items, setItems] = useState<DownloadItem[]>([]);
  const [isProcessing, setIsProcessing] = useState(false);
  const [maxConcurrent, setMaxConcurrent] = useState(1);
  const [connections, setConnections] = useState(4);
  const [modalMsg, setModalMsg] = useState<string | null>(null);
  const cancelRef = useRef(false);
  const speedRef = useRef(new Map<string, { downloaded: number; time: number; ema: number }>());
  const startDownloadsRef = useRef<() => Promise<void>>(async () => {});

  const [activeTab, setActiveTab] = useState<"game" | "updates">("game");
  const [updates, setUpdates] = useState<{ name: string; url: string }[]>([]);
  const [updatesError, setUpdatesError] = useState<string | null>(null);
  const [fetchingUpdates, setFetchingUpdates] = useState(false);
  const [resolvedLinks, setResolvedLinks] = useState<Record<string, ContainerPart[]>>({});
  const [resolving, setResolving] = useState<string | null>(null);
  const [updateItems, setUpdateItems] = useState<DownloadItem[]>([]);
  const [updateProcessing, setUpdateProcessing] = useState(false);
  const updateCancelRef = useRef(false);
  const updateSpeedRef = useRef(new Map<string, { downloaded: number; time: number; ema: number }>());
  const startUpdateDownloadsRef = useRef<() => Promise<void>>(async () => {});

  useEffect(() => {
    document.body.classList.remove("theme-cyberpunk", "theme-gold", "theme-matrix", "theme-retro", "theme-fp");
    document.body.classList.add(`theme-${theme}`);
  }, [theme]);

  useEffect(() => {
    localStorage.setItem("lang", lang);
    localStorage.setItem("theme", theme);
    localStorage.setItem("captchaMode", captchaMode);
  }, [lang, theme]);

  useEffect(() => {
    if (!downloadDir) {
      invoke<string>("default_download_dir")
        .then((dir) => {
          if (dir) {
            setDownloadDir(dir);
            localStorage.setItem("downloadDir", dir);
          }
        })
        .catch(() => {});
    }
  }, [downloadDir]);

  useEffect(() => {
    const tooltip = document.getElementById("global-tooltip");
    const onMouseOver = (e: MouseEvent) => {
      const target = (e.target as Element).closest(".info-icon");
      if (target) {
        const text = target.getAttribute("data-tooltip");
        if (text && tooltip) {
          tooltip.textContent = text;
          tooltip.style.display = "block";
        }
      }
    };
    const onMouseMove = (e: MouseEvent) => {
      if (tooltip && tooltip.style.display === "block") {
        tooltip.style.left = `${e.clientX + 15}px`;
        tooltip.style.top = `${e.clientY + 15}px`;
      }
    };
    const onMouseOut = (e: MouseEvent) => {
      if ((e.target as Element).closest(".info-icon") && tooltip) {
        tooltip.style.display = "none";
      }
    };
    document.addEventListener("mouseover", onMouseOver);
    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseout", onMouseOut);
    return () => {
      document.removeEventListener("mouseover", onMouseOver);
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseout", onMouseOut);
    };
  }, []);

  const fetchUpdates = useCallback(async () => {
    try {
      setUpdatesError(null);
      const url = pageUrl.trim();
      if (!url) return;
      setFetchingUpdates(true);
      const list = await invoke<{ name: string; url: string }[]>("get_updates", { url });
      setUpdates(list);
    } catch (e) {
      setUpdatesError(`Error: ${e}`);
    } finally {
      setFetchingUpdates(false);
    }
  }, [pageUrl]);

  const getContainerLinks = useCallback(async (u: { name: string; url: string }) => {
    try {
      setResolving(u.url);
      const links = await invoke<ContainerPart[]>("open_container", { url: u.url, captchaMode });
      setResolvedLinks((prev) => ({ ...prev, [u.url]: links }));
      setUpdateItems((prev) => {
        const existing = new Set(prev.map((it) => it.link));
        const added: DownloadItem[] = links
          .filter((p) => !existing.has(`${p.url}#${p.name}`))
          .map((p) => ({
            link: `${p.url}#${p.name}`,
            file_name: p.name,
            status: "pending" as const,
            progress: 0,
            checked: false,
          }));
        return added.length > 0 ? [...prev, ...added] : prev;
      });
    } catch (e) {
      setModalMsg(`Error: ${e}`);
    } finally {
      setResolving(null);
    }
  }, [captchaMode]);

  const downloadParts = useCallback((parts: ContainerPart[]) => {
    if (!downloadDir) {
      setModalMsg(t.select_dir_first);
      return;
    }
    const links = new Set(parts.map((p) => `${p.url}#${p.name}`));
    // Mark the passed parts as checked (they were seeded by getContainerLinks).
    setUpdateItems((prev) => {
      const newOnes: DownloadItem[] = parts
        .filter((p) => !prev.some((it) => it.link === `${p.url}#${p.name}`))
        .map((p) => ({
          link: `${p.url}#${p.name}`,
          file_name: p.name,
          status: "pending" as const,
          progress: 0,
          checked: true,
        }));
      const merged = prev.map((it) => (links.has(it.link) ? { ...it, checked: true } : it));
      return newOnes.length > 0 ? [...merged, ...newOnes] : merged;
    });
    // Stay on the Updates tab; download runs in the background.
    setTimeout(() => startUpdateDownloadsRef.current(), 50);
  }, [downloadDir]);

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
      setPageUrl(url);
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
      setModalMsg(t.select_dir_first);
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

  startDownloadsRef.current = startDownloads;

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

  const setUpdateItemStatus = useCallback(
    (idx: number, patch: Partial<DownloadItem>) => {
      setUpdateItems((prev) =>
        prev.map((it, i) => (i === idx ? { ...it, ...patch } : it))
      );
    },
    []
  );

  const pollUpdateProgress = async (link: string, idx: number, startPromise: Promise<string>) => {
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
          setUpdateItemStatus(idx, { status: "resolving", resolverMsg: info.status });
        }
        continue;
      }

      if (resolveErr) {
        if (updateCancelRef.current) {
          setUpdateItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined, resolverMsg: undefined });
        } else {
          setUpdateItemStatus(idx, { status: "error", error: String(resolveErr), resolverMsg: undefined });
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
        updateSpeedRef.current.delete(link);
        if (info.error === "Cancelled") {
          setUpdateItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined, speed: 0, resolverMsg: undefined });
        } else {
          setUpdateItemStatus(idx, { status: "error", error: info.error!, progress: 0, downloaded: 0, speed: 0, resolverMsg: undefined });
        }
        return;
      }

      if (info.paused) {
        setUpdateItemStatus(idx, { status: "paused", speed: 0 });
      }

      if (info.downloaded > 0) {
        const now = Date.now();
        const prev = updateSpeedRef.current.get(link);
        let speed = 0;
        if (prev) {
          const dt = (now - prev.time) / 1000;
          if (dt > 0) speed = (info.downloaded - prev.downloaded) / dt;
        }
        const ema =
          prev && prev.ema > 0
            ? prev.ema * 0.6 + Math.max(0, speed) * 0.4
            : Math.max(0, speed);
        updateSpeedRef.current.set(link, { downloaded: info.downloaded, time: now, ema });
        setUpdateItemStatus(idx, {
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
        updateSpeedRef.current.delete(link);
        break;
      }
    }
  };

  const updateDownloadSingle = useCallback(
    async (link: string, idx: number) => {
      if (!downloadDir || updateCancelRef.current) return;

      setUpdateItemStatus(idx, { status: "resolving" });

      try {
        const startPromise = invoke<string>("start_download", {
          link,
          saveDir: downloadDir,
          parts: connections,
        });

        await pollUpdateProgress(link, idx, startPromise);

        setUpdateItems((prev) => {
          const item = prev[idx];
          if (item && (item.status === "downloading" || item.status === "paused")) {
            return prev.map((it, i) =>
              i === idx ? { ...it, status: "done", progress: 100, error: undefined } : it
            );
          }
          return prev;
        });
      } catch (e) {
        if (updateCancelRef.current) {
          setUpdateItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined, resolverMsg: undefined });
        } else {
          setUpdateItemStatus(idx, { status: "error", error: String(e), resolverMsg: undefined });
        }
      } finally {
        invoke("clear_download", { link }).catch(() => {});
      }
    },
    [downloadDir, connections, setUpdateItemStatus]
  );

  const startUpdateDownloads = useCallback(async () => {
    if (!downloadDir) {
      setModalMsg(t.select_dir_first);
      return;
    }
    const toDownload = updateItems.filter((it) => it.checked && it.status === "pending");
    if (toDownload.length === 0) return;

    updateCancelRef.current = false;
    setUpdateProcessing(true);
    let nextIndex = 0;

    const worker = async () => {
      while (!updateCancelRef.current) {
        const idx = nextIndex++;
        if (idx >= updateItems.length) break;
        const item = updateItems[idx];
        if (!item.checked || item.status !== "pending") continue;
        await updateDownloadSingle(item.link, idx);
      }
    };

    const workers = Array.from({ length: maxConcurrent }, () => worker());
    await Promise.all(workers);

    if (!updateCancelRef.current) {
      setUpdateItems((prev) => prev.map((it) => ({ ...it, checked: false })));
    }
    setUpdateProcessing(false);
  }, [updateItems, downloadDir, maxConcurrent, updateDownloadSingle]);

  startUpdateDownloadsRef.current = startUpdateDownloads;

  const cancelUpdateAll = useCallback(async () => {
    updateCancelRef.current = true;
    const active = updateItems.filter(
      (it) =>
        it.status === "downloading" ||
        it.status === "paused" ||
        it.status === "resolving"
    );
    for (const item of active) {
      await invoke("cancel_download", { link: item.link }).catch(() => {});
    }
    setUpdateProcessing(false);
  }, [updateItems]);

  const pauseUpdateItem = useCallback(async (link: string) => {
    await invoke("pause_download", { link });
  }, []);

  const resumeUpdateItem = useCallback(
    async (link: string, idx: number) => {
      await invoke("resume_download", { link });
      setUpdateItemStatus(idx, { status: "downloading" });
    },
    [setUpdateItemStatus]
  );

  const cancelUpdateItem = useCallback(
    async (link: string, idx: number) => {
      await invoke("cancel_download", { link });
      setUpdateItemStatus(idx, { status: "pending", progress: 0, downloaded: 0, error: undefined });
    },
    [setUpdateItemStatus]
  );

  const toggleUpdateItem = useCallback((link: string) => {
    setUpdateItems((prev) =>
      prev.map((it) => (it.link === link ? { ...it, checked: !it.checked } : it))
    );
  }, []);

  const selectAllUpdateItems = useCallback(() => {
    setUpdateItems((prev) => prev.map((it) => ({ ...it, checked: true })));
  }, []);

  const deselectAllUpdateItems = useCallback(() => {
    setUpdateItems((prev) => prev.map((it) => ({ ...it, checked: false })));
  }, []);

  const downloadCheckedUpdateParts = useCallback((parts: ContainerPart[]) => {
    const checked = parts.filter((p) => {
      const link = `${p.url}#${p.name}`;
      const it = updateItems.find((u) => u.link === link);
      return it ? it.checked : true;
    });
    downloadParts(checked.length > 0 ? checked : parts);
  }, [updateItems, downloadParts]);

  const downloadUpdateItem = useCallback(
    async (link: string) => {
      if (!downloadDir) { setModalMsg(t.select_dir_first); return; }
      const idx = updateItems.findIndex((it) => it.link === link);
      if (idx < 0) return;
      updateCancelRef.current = false;
      setUpdateProcessing(true);
      try {
        await updateDownloadSingle(link, idx);
      } finally {
        setUpdateProcessing(false);
      }
    },
    [downloadDir, updateItems, updateDownloadSingle]
  );

  const anyUpdateActive = updateItems.some(
    (it) => it.status === "downloading" || it.status === "paused" || it.status === "resolving"
  );
  const updateCheckedCount = updateItems.filter((it) => it.checked).length;

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
    <div className="app">
      <div className="top-bar" data-tauri-drag-region>
        <div className="logo-area">
          <div className="logo" id="app-logo">FF<span> Downloader</span></div>
        </div>
        <div className="right-controls-group">
          <div className="app-controls">
            <button
              id="btn-settings"
              className="btn"
              title={t.settings_header}
              onClick={() => setSettingsOpen((v) => !v)}
            >
              <i className="fa-solid fa-gear" />
            </button>
          </div>
          <div className="vertical-separator" />
          <div className="window-controls">
            <button className="win-btn" title="Minimize" onClick={() => invoke("window_minimize")}>
              <i className="fa-solid fa-minus" />
            </button>
            <button className="win-btn" title="Maximize" onClick={() => invoke("window_toggle_maximize")}>
              <i className="fa-regular fa-square" />
            </button>
            <button className="win-btn power-btn" title="Close" onClick={() => invoke("window_close")}>
              <i className="fa-solid fa-power-off" />
            </button>
          </div>
        </div>
      </div>

      <div className="tab-bar">
        <button
          className={activeTab === "game" ? "tab-btn active" : "tab-btn"}
          onClick={() => setActiveTab("game")}
        >
          {t.tab_game}
        </button>
        <button
          className={activeTab === "updates" ? "tab-btn active" : "tab-btn"}
          onClick={() => setActiveTab("updates")}
        >
          {t.tab_updates}
        </button>
      </div>

      <section className="input-section">
        {activeTab === "game" ? (
          <>
            <textarea
              placeholder={t.input_placeholder}
              value={inputText}
              onChange={(e) => setInputText(e.target.value)}
              rows={6}
            />
            <div className="btn-row">
              <button onClick={getLinksFn} disabled={isProcessing}>{t.btn_fetch_url}</button>
              <button onClick={parseLinks} disabled={isProcessing}>{t.btn_parse}</button>
            </div>
          </>
        ) : (
          <>
            <div className="url-row">
              <input
                type="text"
                placeholder={t.url_placeholder}
                value={pageUrl}
                onChange={(e) => setPageUrl(e.target.value)}
              />
              <button onClick={fetchUpdates} disabled={fetchingUpdates}>
                {fetchingUpdates ? t.fetching : t.btn_get_updates}
              </button>
            </div>
            {pageUrl && <div className="url-note">{t.url_note} {pageUrl}</div>}
          </>
        )}
      </section>

      {activeTab === "updates" && (
        <section className="updates-section">
          {updatesError && <div className="updates-error">{updatesError}</div>}
          {updates.length === 0 && !updatesError ? (
            <div className="updates-empty">
              {t.updates_empty}
            </div>
          ) : (
            <div className="updates-list">
              {updates.map((u, i) => (
                <div key={i} className="update-item">
                  <div className="update-row">
                    <span className="update-name">{u.name}</span>
                    <button
                      className="update-links-btn"
                      onClick={() => getContainerLinks(u)}
                      disabled={resolving !== null}
                    >
                      {resolving === u.url ? t.opening : t.btn_get_links}
                    </button>
                  </div>
                  <span className="update-source">{u.url}</span>
                  {resolvedLinks[u.url] && resolvedLinks[u.url].length > 0 && (
                    <div className="resolved-block">
                      <div className="resolved-head">
                        <span>{t.part_s(resolvedLinks[u.url].length)}</span>
                        <div className="resolved-head-btns">
                          {anyUpdateActive ? (
                            <button className="cancel-btn" onClick={() => cancelUpdateAll()} disabled={!updateProcessing && !anyUpdateActive}>
                              {t.btn_cancel_downloads}
                            </button>
                          ) : (
                            <>
                              <button className="resolved-sel-btn" onClick={() => selectAllUpdateItems()} disabled={updateProcessing}>{t.btn_select_all}</button>
                              <button className="resolved-sel-btn" onClick={() => deselectAllUpdateItems()} disabled={updateProcessing || updateCheckedCount === 0}>{t.btn_deselect}</button>
                              <button
                                className="resolved-add-btn"
                                onClick={() => downloadCheckedUpdateParts(resolvedLinks[u.url])}
                                disabled={updateProcessing || updateCheckedCount === 0}
                              >
                                {t.btn_download_all}
                              </button>
                            </>
                          )}
                        </div>
                      </div>
                      <div className="resolved-list">
                        {resolvedLinks[u.url].map((p, j) => {
                          const partLink = `${p.url}#${p.name}`;
                          const item = updateItems.find((it) => it.link === partLink);
                          const uidx = item ? updateItems.findIndex((it) => it.link === partLink) : -1;
                          return (
                            <div key={j} className="resolved-link">
                              <div className="resolved-link-row">
                                <input
                                  type="checkbox"
                                  className="resolved-check"
                                  checked={item ? item.checked : false}
                                  onChange={() => toggleUpdateItem(partLink)}
                                  disabled={item ? item.status !== "pending" && item.status !== "error" : false}
                                />
                                <span className="resolved-name">{p.name}</span>
                                <div className="resolved-link-actions">
                                  {item && item.status === "downloading" && (
                                    <button className="resolved-act-btn" onClick={() => pauseUpdateItem(item.link)}>⏸</button>
                                  )}
                                  {item && item.status === "paused" && (
                                    <button className="resolved-act-btn" onClick={() => resumeUpdateItem(item.link, uidx)}>▶</button>
                                  )}
                                  {item && (item.status === "downloading" || item.status === "paused" || item.status === "resolving") && (
                                    <button className="resolved-act-btn" onClick={() => cancelUpdateItem(item.link, uidx)}>✕</button>
                                  )}
                                  {(!item || (item.status === "pending" || item.status === "error" || item.status === "done")) && (
                                    <button
                                      className="resolved-dl-btn"
                                      onClick={() => downloadUpdateItem(partLink)}
                                      disabled={updateProcessing || (item && item.status === "done")}
                                    >
                                      {item && item.status === "done" ? `✓ ${t.btn_done}` : t.btn_download}
                                    </button>
                                  )}
                                </div>
                              </div>
                              {item && (
                                <div className="resolved-progress">
                                  {item.status === "resolving" && (
                                    <span className="resolved-status">🔎 {t.status_resolving}{item.resolverMsg ? ` (${item.resolverMsg})` : ""}</span>
                                  )}
                                  {item.status === "downloading" && (
                                    <>
                                      <div className="progress-bar">
                                        <div className="progress-fill" style={{ width: `${item.progress}%` }} />
                                      </div>
                                      <span className="progress-text">
                                        {`${item.progress.toFixed(1)}%`}
                                        {item.speed ? ` @ ${formatSpeed(item.speed)}` : ""}
                                      </span>
                                    </>
                                  )}
                                  {item.status === "paused" && <span className="resolved-status">⏸ {t.status_paused}</span>}
                                  {item.status === "done" && <span className="resolved-status">★ {t.status_done}</span>}
                                  {item.status === "error" && <span className="resolved-status">✖ {item.error}</span>}
                                </div>
                              )}
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {items.length > 0 && activeTab === "game" && (
        <section className="items-section">
          <div className="items-header">
            <span>{t.lbl_files(items.length)}</span>
            <div className="items-header-btns">
              {!anyActive && (
                <>
                  <button onClick={selectAll} disabled={isProcessing} className="sel-btn">{t.btn_select_all}</button>
                  <button onClick={deselectOptional} disabled={isProcessing || !optionalChecked} className="sel-btn">{t.btn_deselect_optional}</button>
                  <button onClick={deselectAll} disabled={isProcessing || checkedCount === 0} className="sel-btn">{t.btn_deselect_all}</button>
                </>
              )}
              <button
                onClick={anyActive ? cancelAll : startDownloads}
                disabled={!anyActive && !canDownload}
                className={anyActive ? "cancel-btn" : "start-btn"}
              >
                {anyActive ? t.btn_cancel_downloads : t.btn_download}
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
                          if (!downloadDir) { setModalMsg(t.select_dir_first); return; }
                          cancelRef.current = false;
                          setIsProcessing(true);
                          await downloadSingle(item.link, idx);
                          setIsProcessing(false);
                        }}
                      >
                        {t.btn_download}
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

      <div id="settings-view" className={`settings-screen${settingsOpen ? " settings-visible" : ""}`}>
        <div className="settings-header">
          <h2 id="modal-title"><i className="fa-solid fa-sliders" /> <span>{t.settings_header}</span></h2>
          <button id="btn-close-settings-top" className="icon-btn" onClick={() => setSettingsOpen(false)}>
            <i className="fa-solid fa-times" />
          </button>
        </div>

        <div className="settings-scroll">
          <details open>
            <summary><i className="fa-solid fa-folder-open" /> <span>{t.settings_general}</span></summary>
            <div className="settings-group file-paths">

              <div className="settings-row">
                <span className="control-label tooltip-wrapper">{t.settings_language}
                  <span className="info-icon" data-tooltip={t.tooltip_language}><i className="fa-solid fa-question" /></span>
                </span>
                <div className="control-value">
                  <select value={lang} onChange={(e) => setLang(e.target.value as LangCode)}>
                    {LANGS.map((l) => <option key={l.code} value={l.code}>{l.name}</option>)}
                  </select>
                </div>
              </div>

              <div className="settings-row">
                <span className="control-label tooltip-wrapper">{t.settings_theme}
                  <span className="info-icon" data-tooltip={t.tooltip_theme}><i className="fa-solid fa-question" /></span>
                </span>
                <div className="control-value">
                  <select value={theme} onChange={(e) => setTheme(e.target.value as ThemeName)}>
                    <option value="cyberpunk">{t.theme_cyberpunk}</option>
                    <option value="gold">{t.theme_gold}</option>
                    <option value="matrix">{t.theme_matrix}</option>
                    <option value="retro">{t.theme_retro}</option>
                    <option value="fp">{t.theme_fp}</option>
                  </select>
                </div>
              </div>

              <div className="settings-row">
                <span className="control-label tooltip-wrapper">{t.settings_download_dir}
                  <span className="info-icon" data-tooltip={t.tooltip_download_dir}><i className="fa-solid fa-question" /></span>
                </span>
                <div className="control-value path-control">
                  <span className="path-display">{downloadDir || t.settings_not_selected}</span>
                  <button className="btn sm" onClick={pickDir}>{t.settings_select}</button>
                </div>
              </div>

              <div className="settings-row">
                <span className="control-label tooltip-wrapper">{t.settings_max_concurrent}
                  <span className="info-icon" data-tooltip={t.tooltip_max_concurrent}><i className="fa-solid fa-question" /></span>
                </span>
                <div className="control-value">
                  <select value={maxConcurrent} onChange={(e) => setMaxConcurrent(Number(e.target.value))}>
                    <option value={1}>1</option>
                    <option value={2}>2</option>
                    <option value={3}>3</option>
                    <option value={4}>4</option>
                    <option value={5}>5</option>
                  </select>
                </div>
              </div>

              <div className="settings-row">
                <span className="control-label tooltip-wrapper">{t.settings_connections}
                  <span className="info-icon" data-tooltip={t.tooltip_connections}><i className="fa-solid fa-question" /></span>
                </span>
                <div className="control-value">
                  <select value={connections} onChange={(e) => setConnections(Number(e.target.value))}>
                    <option value={1}>1</option>
                    <option value={2}>2</option>
                    <option value={3}>3</option>
                    <option value={4}>4</option>
                    <option value={6}>6</option>
                    <option value={8}>8</option>
                  </select>
                </div>
              </div>

              <div className="settings-row">
                <span className="control-label tooltip-wrapper">{t.settings_captcha}
                  <span className="info-icon" data-tooltip={t.tooltip_captcha}><i className="fa-solid fa-question" /></span>
                </span>
                <div className="control-value">
                  <select value={captchaMode} onChange={(e) => setCaptchaMode(e.target.value as "auto" | "manual")}>
                    <option value="auto">{t.captcha_auto}</option>
                    <option value="manual">{t.captcha_manual}</option>
                  </select>
                </div>
              </div>

            </div>
          </details>
        </div>

        <div className="modal-footer">
          <button className="btn primary" onClick={() => setSettingsOpen(false)}>
            <i className="fa-solid fa-save" /> <span>{t.btn_save}</span>
          </button>
          <button className="btn" onClick={() => setSettingsOpen(false)}>
            <i className="fa-solid fa-xmark" /> <span>{t.btn_close}</span>
          </button>
        </div>
      </div>

      {modalMsg && (
        <div className="modal-overlay" onClick={() => setModalMsg(null)}>
          <div className="modal-box">
            <div className="modal-icon">⚠</div>
            <div className="modal-text">{modalMsg}</div>
            <button className="modal-btn" onClick={() => setModalMsg(null)}>{t.ok}</button>
          </div>
        </div>
      )}

      <div id="global-tooltip" style={{ display: "none" }} />
    </div>
  );
}

export default App;
