# Changelog

**English** · [Español](CHANGELOG.es.md)

## v1.3.2

- **Media panel size**: new *Media panel size* menu (compact 400, medium 480, large 560 — default —, extra large 680, maximum 800 logical px) and `MediaWidth` key for any width from 320 to 1200. The panel still shrinks if the free space is smaller.
- New **Claude** icon: the pixel-art Claude Code mascot (terracotta) instead of the ray star.
- **All GPUs and the NPU** in the system flyout: one card per GPU (NVIDIA green, AMD red, Intel blue) plus the NPU (Intel AI Boost, AMD XDNA, Qualcomm Hexagon) in violet. **AMD Radeon and Intel support**: usage and memory come from the Windows GPU counters (the same ones Task Manager uses) and temperature, fan and clock from D3DKMT; NVML still adds power draw on NVIDIA. The taskbar panel shows the main GPU (dedicated, most VRAM). New `--gpu-report <file>` switch that writes the detected GPUs/NPUs and a reading, for troubleshooting.
- **Vertical taskbars** (left/right): the system, AI and network panels are icons by default; choosing *Panel* in *Panel format* keeps the detailed vertical layout.
- **Left-aligned taskbar icons**: the media panel no longer covers the icons (the Start button can start exactly at the taskbar edge, which was discarded and the taskbar was taken as centered). In automatic mode, the system and AI panels now switch to icons when the full panels would not leave room for the whole media panel between the icons and them.
- The panels in icon mode no longer expand when a UI Automation query of the taskbar fails (e.g. with the taskbar context menu open): the last known icon positions are kept.
- **Three new spectrum styles**: segmented LED (neon colors like the other styles, with held peaks), mirrored bars (bars over a baseline with a faded reflection) and neon line (glowing curve with a gradient fill). Five styles in total, in the *Spectrum style* menu (`SpectrumStyle`: `wave`, `bars`, `led`, `mirror`, `line`).
- **Lower memory use**: the resident private memory at rest drops from ~31 MB to ~5 MB. The app releases pages that are only touched at startup (NVML, UI Automation, WinRT) 45 s after launch, frees the flyout canvas 15 s after it closes and hidden panels' canvases, uses one shared text buffer instead of a full-size copy per canvas, and caches GDI fonts instead of creating one per text draw.
- **Documentation in English by default**: `README.md` and `CHANGELOG.md` are now in English; the Spanish versions are `README.es.md` and `CHANGELOG.es.md`.
- **Panel format** (menu and `PanelStyle` key): the system and AI panels can be automatic (icon only if it does not fit, as before), full panel or icon; the network panel, panel or icon (new icon mode with download and upload in short form over the chart).
- New **GitHub Copilot** icon: blue → purple gradient helmet, pilot goggles and a dark visor with cyan eyes (instead of the white face on a purple circle).
- **Panels no longer disappear while the Start menu is open**: they are now child windows of the taskbar (previously, floating "always on top" windows that Windows kept below the taskbar while Start was open). They follow the taskbar, are recreated automatically if Explorer restarts and, if Windows does not allow embedding them, fall back to floating mode. The executable now embeds an application manifest (Windows 8+ compatibility), required for transparent child windows.

## v1.3.1

- **Copilot**: the GitHub token is looked up in several sources (manually saved token, `GH_TOKEN`/`GITHUB_TOKEN`, official Copilot plugins, GitHub CLI and Git Credential Manager) and the first one with Copilot access is used. New **AI → GitHub token for Copilot…** menu to save it manually (Credential Manager). The status tells apart "no credentials", "token has no Copilot access" and "offline".

## v1.3.0

- **Installer** `TaskbarPanels-Setup-vX.Y.Z-x64.exe` (Inno Setup): installs to *Program Files*, downloads and verifies PresentMon (optional task), registers the scheduled task and adds the uninstaller to *Settings → Apps*. Supports silent installation (`/VERYSILENT`).
- **winget**: available as `sardilac.TaskbarPanels`; every release automatically opens the update in `microsoft/winget-pkgs`.
- **English documentation** (`README.en.md`, `CHANGELOG.en.md`).

## v1.2.1

- **Country flag** of the public IP to the left of the IP (network flyout and the panel's IP row). It is downloaded once per country from `flagcdn.com` and cached in `%APPDATA%\TaskbarPanels\flags`. The country comes from Cloudflare (`cdn-cgi/trace`) when it disagrees with ifconfig.co, which can misplace IP blocks announced from another country.
- **Speed test with four providers**: Speedtest.net (Ookla, default; picks the nearby server with the lowest latency), Cloudflare, LibreSpeed (open source; public server with the lowest latency) and GCI (`speedtest.gci.co.cr`, a LibreSpeed server in Costa Rica). Switch it with the card's selector (click to move to the next one) or from the *Speed test provider* menu (`SpeedTestProvider` key). The card shows the server used and, on failure, the reason.
- Speed test: if the server rate-limits large downloads (Cloudflare 429 after several tests in a row) it retries with smaller chunks; the upload phase no longer lingers a few extra seconds waiting for connections to close.
- Panels hide when the taskbar is not visible on screen: besides detecting full-screen foreground windows, the window over several points of the taskbar is checked. A full-screen game (e.g. Fortnite) no longer has the panels on top even when focus is on another display.

## v1.2.0

- **Network · speed test**: new card in the network flyout with a button to measure the connection against `speed.cloudflare.com`: latency and jitter, download and upload (4 parallel connections, ~8 s per phase) with a live gauge, progress bar and Cloudflare data center. It only runs when the button is pressed and uses up to ~350 MB.
- **Vertical taskbars**: the media panel sits at the top, at the start of the taskbar (below the Start button if it is at the top), and only moves below the icons if it does not fit.

## v1.1.0

- **AI with multiple providers**: Claude, GitHub Copilot, OpenAI API (month spend vs. budget), Anthropic API (Admin API) and custom providers (JSON URL). Each one with its icon; choose the **primary provider** shown on the taskbar (or a summary of all). *AI* menu: primary provider, enable/disable, add (form) and remove. Keys are stored in Windows Credential Manager.
- **Icon mode**: if the app icons reach the panels, the system and AI panels shrink to an icon (CPU ring / provider icon with its value).
- **Language**: the interface follows the Windows language (Spanish if the system is in Spanish; English otherwise), with matching number and date formats. It can be forced from the *Idioma / Language* menu or with `"Language": "es" | "en" | "auto"`.
- **Media**: the panel only shows while there is a media session (playing or paused), with a 3 s grace period; it is more compact (560 logical px) and keeps a 16 px margin from the taskbar icons, which are now located with UI Automation from a background thread.
- Configurable **flyout background**: frosted glass, clear glass, Windows acrylic, mica or dark (*Window background* menu, `PopupStyle` key).
- **Network**: new card with an animated router, gateway, DNS, adapter and gateway ping with chart and statistics; the ping only runs while the flyout is open.
- The media flyout uses the chosen spectrum style (wave or bars).
- Panels no longer hide because of shell windows (tray flyouts, Start menu, notifications) or DWM-cloaked windows; hiding also requires the condition to last ~0.8 s.
- Panels no longer disappear for ~1 s when clicking the taskbar (e.g. *Show hidden icons*): they are raised above the taskbar as soon as the foreground window changes.
- Installer: waits for the previous version to close and retries the copy (an update could previously leave it not running).

## v1.0.0

First public release.

- **Network**: upload/download speed with 60 s charts, public IP with status indicator and IP-change notification; option to hide the IP.
- **AI usage**: Claude usage (5-hour session and weekly) and GitHub Copilot (premium requests); show one, the other or both.
- **System**: CPU, GPU, temperatures, FPS (PresentMon) and disks (read/write and system-drive ring).
- **Media**: album art with an animated vinyl, buttons, scrolling title, progress and audio spectrum (liquid wave or capsule bars).
- Frosted-glass flyouts with the details of each panel.
- Multiple displays (all, primary only or a selection), small, top or vertical taskbars, left-aligned or centered icons.
- Hidden while full-screen applications are running.
