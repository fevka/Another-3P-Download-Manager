# Third-Party Download Manager

A desktop application for Tauri v2 that automates file downloads from supported third-party download hosts.

The application can extract supported download links from a provided webpage, assist with browser-based access to protected download pages, resolve available download URLs, and download files using parallel segmented connections.

Built with **Rust** (Tauri v2, `wreq` HTTP client with Firefox 148 emulation) on the backend and **React 18 + TypeScript + Vite** on the frontend.

> **Legal Notice**
>
> This software is a general-purpose download automation tool. It does not host, store, or distribute files obtained from third-party services.
>
> The software is intended to be used only with content that you are legally entitled or otherwise authorized to access and download.
>
> Users are solely responsible for ensuring that their use of the software complies with applicable laws, copyright requirements, and the terms and conditions of any third-party website or service they access.
>
> The developers do not endorse or encourage copyright infringement, unauthorized access, or violations of third-party terms of service.
>
> No representation is made that any particular third-party website, host, or content is compatible with or authorized for use with this software.

---

## Table of Contents

* [Features](#features)
* [How It Works](#how-it-works)
* [Prerequisites](#prerequisites)
* [Development](#development)
* [Building a Release](#building-a-release)
* [Usage](#usage)
* [Options](#options)
* [Controls](#controls)
* [Debug Logging](#debug-logging)
* [Troubleshooting](#troubleshooting)
* [Project Structure](#project-structure)
* [License](#license)
* [Legal Notice](#legal-notice)

---

## Features

* **One-click setup** — paste a supported webpage URL or a list of direct download-host links; the app extracts available file links automatically.
* **Browser-assisted access** — a hidden WebView2 window can interact with protected download pages using normal browser behavior and obtain an available download redirect.
* **Parallel segmented downloads** — each file is split into `N` ranges (default 4, up to 8) downloaded concurrently to maximize available bandwidth.
* **Resilience** — per-part automatic retries (5 attempts) that resume from the last written offset, plus a 20 s idle timeout that detects and recovers from stuck/connection-hung connections.
* **Queue management** — download up to 5 files concurrently, with per-item **pause / resume / cancel** controls.
* **Cleanup on cancel** — cancelling a download deletes the partially written file.
* **Live progress** — per-file progress bar, MB downloaded, total size, and a smooth (EMA-smoothed) transfer-rate readout.
* **Selective download** — choose individual files, select/deselect all, or skip optional files in one click.
* **Cross-platform packaging** — builds Windows installers (MSI + NSIS) via Tauri bundler.

---

## How It Works

### 1. Link extraction

Given a supported webpage URL, the application fetches the HTML and looks for recognized download-host links.

If additional links are contained inside supported expandable sections, those links are extracted, sorted naturally, and deduplicated.

Each extracted link is passed to the download-resolution stage.

---

### 2. Download resolution

Some download hosts use browser-based protection or session mechanisms that prevent a normal HTTP request from immediately returning a downloadable file.

For supported hosts, the application opens a **WebView2 window** and interacts with the page using browser behavior.

The resolver:

1. Loads the supplied download page.
2. Waits for the page's download controls to become available.
3. Interacts with the page as required to initiate the download.
4. Waits for the resulting session information.
5. Obtains the download redirect provided by the host.
6. Passes the resulting download URL to the Rust backend.

The resolver window can remain hidden during normal operation.

If automatic resolution does not complete within **180 seconds**, the window becomes visible so the user can complete any required browser interaction manually.

> The application does not provide or host the protected content itself. It only automates the browser/download workflow supported by the third-party service.

---

### 3. Parallel download

The backend probes the file size with a `Range: bytes=0-0` request and, when supported by the server, pre-allocates the file and splits the download into multiple ranges.

Each part:

* Opens its own file handle and seeks once to its start offset.
* Streams its range sequentially.
* Updates global progress every 256 KB.
* Retries from the last written byte after a connection loss or prolonged inactivity.
* Performs up to 5 retry attempts with backoff.

---

## Prerequisites

| Requirement                                                                                  | Version / Notes                    |
| -------------------------------------------------------------------------------------------- | ---------------------------------- |
| [Node.js](https://nodejs.org)                                                                | ≥ 18 (for frontend build)          |
| [Rust toolchain](https://rustup.rs)                                                          | stable, recent                     |
| [Tauri CLI](https://tauri.app)                                                               | v2 (`@tauri-apps/cli`)             |
| [NASM](https://www.nasm.us)                                                                  | **required** to compile `btls-sys` |
| [Microsoft WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) | required at runtime                |

---

## Development

```bash
# Install frontend dependencies
npm install

# Run the app in development mode
npm run tauri dev

# Or run the frontend alone
npm run dev
```

The Vite development server listens on:

```text
http://localhost:1420
```

---

## Building a Release

```bash
# 1. Build the frontend
npm run build

# 2. Build and bundle the application
npm run tauri build
```

Output:

* Executable: `src-tauri/target/release/ff-downloader.exe`
* Installers: MSI + NSIS under `src-tauri/target/release/bundle/`

---

## Usage

1. **Launch the application.** Select a save directory with **BROWSE**. The selection is remembered for future sessions.

2. **Enter links.** Paste either:

   * A supported webpage URL, or
   * A list of supported direct download-host links.

3. Click **PARSE** to load the available files into the list.

4. **Choose which files to download.**

   * **SELECT ALL** — select every available file.
   * **DESELECT OPTIONAL** — deselect files whose names contain `optional`.
   * **DESELECT ALL** — clear the current selection.

5. **Configure the download** before starting:

   * **Max:** number of files downloaded concurrently (1–5, default 1).
   * **Conn:** number of parallel connections per file (1, 2, 3, 4, 6, 8 — default 4).

6. Click **DOWNLOAD**.

Each file progresses through:

* `resolving` — the browser-based download resolution is performed.
* `downloading` — file ranges are fetched in parallel.
* `done` — the download completed successfully.

Files are written using the filename supplied by the download source.

---

## Options

| Option   | Values           | Default | Description                                      |
| -------- | ---------------- | ------- | ------------------------------------------------ |
| **Max**  | 1 – 5            | 1       | Maximum number of files downloaded concurrently. |
| **Conn** | 1, 2, 3, 4, 6, 8 | 4       | Number of parallel range connections per file.   |

More connections may improve throughput on some servers, while other servers may throttle or limit concurrent connections.

---

## Controls

While a file is downloading:

* **Pause** — temporarily pause that file.
* **Resume** — continue a paused file.
* **Cancel (✕)** — stop the download and remove the partial file.

The main button becomes **CANCEL DOWNLOADS** while downloads are active and cancels the current queue.

---

## Debug Logging

The application writes a timestamped log to:

```text
%TEMP%\ff_debug.txt
```

Log entries may include:

* resolver navigation events
* download redirects
* transfer-rate samples
* per-part retry information
* manually completed browser interactions

On a Rust panic, a crash dump may be written to:

```text
%TEMP%\ff_panic.txt
```

> Debug logs may contain URLs or other technical information generated during the download process. Review or remove these logs if you intend to share them publicly.

---

## Troubleshooting

| Symptom                                     | Likely cause / fix                                                                                                                      |
| ------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `Cloudflare/DDoS protection`                | The third-party service may require browser-based verification. Restart the download or complete the verification manually if prompted. |
| Download starts but seems stuck             | A connection may be hanging. The idle timeout detects this and retries from the last written byte.                                      |
| A browser window appears after ~3 minutes   | Automatic browser interaction timed out. Complete the required interaction manually.                                                    |
| Resolver errors                             | The third-party service may not have returned a valid download redirect. Try again later or use manual interaction if available.        |
| Slow / fluctuating speed                    | Try increasing **Conn**, or lowering it if the server limits concurrent connections.                                                    |
| Cancel does not immediately delete the file | Existing connections may still be closing. Cleanup is retried automatically.                                                            |
| `btls-sys` build failure                    | NASM may be missing from `PATH`. Install it and rebuild.                                                                                |

---

## Project Structure

```text
ff-downloader/
├── src/                        # React frontend
│   ├── App.tsx                 # UI, queue, polling, speed display
│   ├── App.css                 # styles
│   └── main.tsx                # React entry
├── src-tauri/
│   ├── src/lib.rs              # backend logic
│   ├── Cargo.toml              # dependencies
│   └── tauri.conf.json         # application configuration
└── package.json                # frontend scripts and dependencies
```

---

## Key Backend Components

| Symbol                 | Purpose                                                   |
| ---------------------- | --------------------------------------------------------- |
| `get_links_from_page`  | Extracts supported download links from a webpage.         |
| `resolve_via_webview`  | Performs browser-based download resolution.               |
| `resolve_download_url` | Resolves an available download URL from a supported host. |
| `probe_total`          | Determines the downloadable file size when supported.     |
| `download_part`        | Downloads one range and handles retries.                  |
| `parallel_download`    | Splits a file into ranges and manages parallel downloads. |
| `single_download`      | Fallback single-stream download.                          |
| `start_download`       | Main download workflow.                                   |

---

## License

This project is provided under the license included with the repository.

Third-party websites, download hosts, trademarks, and copyrighted works referenced by this project remain the property of their respective owners.

This project is not affiliated with, endorsed by, or sponsored by any third-party website or download service unless explicitly stated otherwise.

---

## Legal Notice

This software is provided as a download automation utility.

The developers do **not**:

* host downloaded files;
* provide storage for downloaded content;
* distribute copyrighted files through the application;
* claim ownership of third-party content;
* guarantee the availability of any third-party service;
* guarantee that use of the application is permitted by a particular website.

The application may interact with third-party websites and services that have their own terms, technical restrictions, access controls, and usage policies.

Users are responsible for:

1. Ensuring that they have the necessary rights or authorization to access and download the content they request.
2. Complying with applicable copyright and other intellectual-property laws.
3. Complying with the terms and policies of the websites and services they access.
4. Ensuring that their use of the software does not violate applicable laws or contractual restrictions.

The developers do not encourage unauthorized access, copyright infringement, circumvention of access restrictions, or violation of third-party terms of service.

**Use of this software does not grant the user any rights to third-party content.**

To the extent permitted by applicable law, the software is provided **"as is"**, without warranties regarding the availability, legality, accuracy, or suitability of third-party content or services.

---

## Important

Nothing in this README should be interpreted as legal advice.

If you are unsure whether a particular use of the software is permitted in your jurisdiction or under the terms of a third-party service, obtain appropriate legal advice or do not use the software for that purpose.
