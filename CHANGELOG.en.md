# Changelog

[Español](CHANGELOG.md) · **English**

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
