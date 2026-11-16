# Changelog

**English** · [Español](CHANGELOG.es.md)

## v1.5.2

- **CPU usage matches Task Manager**: on Windows 11 24H2 and later the system panel now reads the standard *% Processor Time*, as Task Manager does. It used to read *% Processor Utility*, which is weighted by clock speed and with turbo boost showed about twice as much (for example 43 % against 21 %). Earlier Windows versions keep *% Processor Utility*, which is what their Task Manager shows.
- **Icon mode, panel by panel**: in automatic format only the panels that actually reach the app icons collapse to an icon. Dragging one panel (the radio, for instance) up to the icons used to collapse every other panel too, even with plenty of room. With centered icons, a panel dragged to the free space left of the Start button keeps its full format.
- **Radio flyout**: fixed, it sometimes stayed open after the search box had been used. The keyboard focus is now checked for real and only keeps the flyout open while you are typing (6 s after the last key or click once the pointer has left).

## v1.5.1

- **Narrower radio panel**: it now ends where the **ON AIR** sign ends (94 px instead of 176) and the station name sits below it as a **banner** that scrolls when it does not fit. While connecting, the sign shows `• • •`.
- **Network in icon mode**: it is now a **circle** like the system one: two concentric rings (download outside, upload inside, each relative to its last-minute peak) with the download speed below.
- **Network in automatic format**: the network panel also collapses to an icon when the taskbar fills up with windows, like system, AI and radio. It is the new default; choose it under *Panel format → Network: automatic* (`"NetworkPanel": "auto"`).
- **Speed test**:
  - The measurement no longer stops halfway on fast connections: each phase always lasts 8 s (it used to stop at 250 MB of download or 100 MB of upload). Data use now grows with speed: about 150 MB on a symmetric 100 Mb/s line, close to 1.5 GB on 1 Gb/s.
  - Fixed: the **Start test** button stopped responding after the radio flyout had been opened.

## v1.5.0

- **Radio panel** (new): Internet radio stations from every country in the Americas and Europe (Radio Browser directory; each country catalog is cached and refreshed daily). On the taskbar it shows an **ON AIR** sign with the station name while it plays and, in icon mode, a red musical note. The popup lets you pick the country first and then the station, search by name, city or genre (also across the full directory of the country), stop or resume, adjust the volume and see the current station details. The station also shows up in the Windows media controls (and in the media panel, with its spectrum). Enable or disable it under *Panels → Radio*; choose its format (automatic, panel or icon) under *Panel format*. Diagnostics: `taskbar-panels.exe --radio-report <file>`.

## v1.4.0

- **Connection health** (network panel): the router, the DNS server and the Internet are pinged every 2 s and compared hop by hop to tell where a problem is — *Unstable Wi-Fi*, *Router not responding*, *No Internet access*, *Internet dropping packets*, *High Internet latency* or *Slow DNS*. The panel shows a status dot (green, amber or red with a glow); the flyout adds a card with the diagnosis and a 2-minute timeline of latency and loss per hop. Targets that never answer pings are left out instead of raising false alarms. Can be turned off from the menu (*Connection health*) or with `"NetHealth": false`.
- **More AI providers** (menu *AI → Add provider*):
  - **DeepSeek** and **Kimi (Moonshot AI)**: account balance; with an optional reference balance (your last top-up) the bar shows how much has been used. Kimi accepts the Chinese platform server (`https://api.moonshot.cn`, balance in yuan).
  - **Ollama (local)** and **LM Studio**: models loaded in memory out of the installed ones, with their names (and memory in use for Ollama). Checked every 30 s; the card says when the server is not running. The server address can be changed to reach another machine.
  - **Gemini** and **Ollama Cloud**: neither publishes usage or remaining quota, so the key is checked and the available models are counted.
  - New drawn icons for each one. Indicators without a percentage (balance, model counts) use the provider color and no empty bar.
- **Fans and power** (system flyout):
  - **Fans**: a card with an animated rotor for the CPU and the GPU fan that spins in proportion to the real rpm, next to the value. Shown only on machines that expose fan speed.
  - **Battery**: level with color by charge, a shimmer running across it while charging, charge or discharge power in watts and the estimated time left. Desktops show *No battery*.
  - **Power plan**: name of the active plan and, on Windows 11, the power mode (*Best efficiency* / *Best performance*), with arrows that advance slowly in green for power saving, in blue for balanced and fast in orange for high performance.
- **System status dot** (system panel): green when all is well, amber or red with a glow when something needs attention, like the network one. It watches CPU and GPU temperature (only when it stays high for about 30 s, not on a spike), memory in use (90 % / 96 %), free space on the system drive (10 % / 5 %) and battery level when unplugged (15 % / 7 %). The flyout lists the active alerts at the top.
- **Top consumers** (system flyout): the five apps using the most CPU and memory right now, with the processes of the same program added together. Sampled only while the flyout is open.
- **System flyout in two columns**: CPU, GPUs, NPU and fans on the left; memory, drives, power, apps and FPS on the right. Both columns always end at the same height.
- **Lower CPU and memory use**:
  - Flyouts repaint only their animated parts (router; fans and power) between the once-per-second full repaints, instead of the whole window on every frame.
  - The media panel is drawn at 30 fps while playing (the spectrum is analyzed at 30 Hz) and at 10 fps while paused, and not at all when no media panel is visible. Spectrum smoothing no longer depends on the frame rate.
  - Taskbar colors are read from the screen only for 20 s after a Windows mode change or when a taskbar appears, and every 30 s otherwise, instead of every second.
  - Text rendering skips empty pixels and uses a precomputed coverage table; the flyout background is no longer duplicated in memory on every repaint.
  - The taskbar icon watcher polls every 1.5 s while the taskbar is stable (0.7 s for 10 s after a change), and the tray tooltip is updated only when its text changes.
  - With `debug.flag`, `debug.log` now records CPU time per thread and per section of the interface thread every 10 s.

## v1.3.4

- **Light and dark mode**: panels and flyouts follow the Windows mode and switch on the fly. In light mode every color is converted to its dark counterpart (perceptual OKLab lightness inverted, hue kept), so text, charts and indicators stay readable on a light taskbar; flyouts use a light tint and DWM light mode. Provider icons, album art and the vinyl keep their own colors.
- **Each taskbar is checked by its actual on-screen color**: Explorer sometimes leaves a taskbar (usually a secondary one) dark after switching modes; panels on that taskbar keep the dark palette instead of becoming unreadable.
- The *Dark* flyout background is now *Solid* and follows the mode (`PopupStyle` stays `"dark"`).
- The speed-test button uses the accent blue with white text in light mode.
- `--render-demo` also generates the light-mode images (`*-light.png`).

## v1.3.3

- **Network, AI and system panels can be dragged independently**: each one keeps its own position on the taskbar (previously only the network panel could be dragged and it moved all three together). A panel that reaches another one pushes it, never overlapping; dragging past a neighbor swaps their order. No panel leaves the taskbar or covers the media panel's space, which moves aside or hides when there is no room. `pos.json` now stores per-panel `Offsets` (the old `Gap` is migrated automatically) and *Reset position* groups them back on the right.
- **Narrower system panel** (236 → 150 logical px): more compact CPU/GPU/FPS charts and no drive free-space and read/write text; the ring with the % used remains (details are still in the flyout).
- **Narrower network panel** (112 → 96 logical px); an IP that doesn't fit is cut with "…".

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
