# Taskbar Panels

**Español** · [English](README.en.md)

Paneles modernos y ligeros para la barra de tareas de **Windows 11**, escritos en **Rust**:
red, consumo de IA (Claude / GitHub Copilot), sistema (CPU, GPU, FPS, discos) y multimedia.
Transparencia real, animaciones a 60 fps y ventanas flotantes de vidrio esmerilado con el
detalle de cada panel, todo en un solo proceso de ~60 MB de RAM y menos del 1 % de un núcleo.

![Barra de tareas con los cuatro paneles](docs/images/barra.png)

> Las imágenes se generan con datos de ejemplo (`taskbar-panels.exe --render-demo`).

## Paneles

| Panel | En la barra | Al pasar el mouse |
|---|---|---|
| **Multimedia** (si hay algo reproduciéndose o en pausa) | Carátula con vinilo animado, botones anterior / reproducir / siguiente con el color del álbum, título desplazable, progreso y espectro de audio (onda líquida o barras cápsula) | Carátula grande, título, artista, álbum, app, tiempos y espectro |
| **Sistema** | CPU y GPU (uso + temperatura), FPS de la ventana activa y anillo del disco del sistema con lectura/escritura | Modelo de CPU/GPU, frecuencias, ventiladores, VRAM, consumo, RAM, todas las unidades en anillos y E/S de disco con gráficas |
| **Consumo de IA** | El proveedor principal con su icono (o un resumen de todos) | Una tarjeta por proveedor: icono, plan, barras, gasto y reinicios (hora local) |
| **Red** | Subida y bajada con gráficas de 60 s, IP pública con indicador de estado | ISP, ubicación, router con puerta de enlace, DNS, adaptador y **ping a la puerta de enlace** (solo mientras la ventana está abierta), máximos del último minuto, totales de la sesión y **speed test** bajo demanda (Speedtest.net, Cloudflare, LibreSpeed o GCI) y bandera del país de la IP |

<p>
  <img src="docs/images/popup-sistema.png" width="300" alt="Ventana flotante de sistema">
  <img src="docs/images/popup-multimedia.png" width="300" alt="Ventana flotante de multimedia">
</p>
<p>
  <img src="docs/images/popup-red.png" width="300" alt="Ventana flotante de red">
  <img src="docs/images/popup-consumo.png" width="300" alt="Ventana flotante de consumo">
  <img src="docs/images/barra-vertical.png" height="420" alt="Barra vertical">
</p>

**Se adapta a la barra:** si hay muchas apps abiertas, los paneles de sistema e IA se comprimen a un icono; varias pantallas (todas, solo la principal o una selección por monitor),
barra pequeña o normal, barra arriba, barras verticales (izquierda/derecha, con mods como Windhawk),
iconos centrados o a la izquierda y escala por monitor. Los paneles se ocultan siempre que la barra
no se ve (juegos o videos a pantalla completa, barra auto-ocultada).

## Requisitos

- Windows 11 x64 (Windows 10 puede funcionar, sin garantías).
- Permisos de administrador para instalar: el programa corre con privilegios elevados (tarea
  programada) porque los sensores de temperatura y PresentMon lo necesitan.

Funciones que dependen del equipo (si no están disponibles, el panel muestra «—»):

| Dato | Necesita |
|---|---|
| Uso, temperatura, VRAM, consumo y frecuencia de GPU | GPU **NVIDIA** (NVML, incluido en el driver) |
| Temperatura de CPU y velocidad de ventiladores | Portátil **Lenovo Legion/LOQ** (WMI `LENOVO_OTHER_METHOD`) |
| FPS | [PresentMon](https://github.com/GameTechDev/PresentMon) de Intel (el instalador lo descarga) |
| Consumo de Claude | Sesión iniciada en **Claude Code** en ese usuario |
| Consumo de Copilot | Credencial de GitHub guardada por **Git Credential Manager** con Copilot activo |
| Gasto de la API de OpenAI / Anthropic | Una **clave de administrador** de cada plataforma (se pide al agregar el proveedor) |

## Instalación

### winget (recomendado)

```powershell
winget install sardilac.TaskbarPanels
```

Actualizar: `winget upgrade sardilac.TaskbarPanels` · desinstalar: `winget uninstall sardilac.TaskbarPanels`.

### Instalador

Descarga `TaskbarPanels-Setup-vX.Y.Z-x64.exe` desde [Releases](../../releases) y ejecútalo (pide UAC).
Instalación desatendida: `TaskbarPanels-Setup-vX.Y.Z-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`
(añade `/TASKS=""` para no descargar PresentMon).

El instalador:
- copia el programa a `C:\Program Files\TaskbarPanels`;
- descarga PresentMon del release oficial de Intel y **verifica su firma digital** (tarea opcional);
- registra la tarea programada **TaskbarPanels** (al iniciar sesión, con privilegios elevados, se reinicia
  sola si falla) para el usuario que instala, y arranca los paneles;
- agrega el desinstalador a *Configuración → Aplicaciones*.

### Zip + PowerShell (alternativa)

1. Descarga `TaskbarPanels-vX.Y.Z-x64.zip` desde [Releases](../../releases) y descomprímelo.
2. Abre PowerShell en esa carpeta y ejecuta:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   Hace lo mismo que el instalador (usa `-SkipPresentMon` para omitir PresentMon).

> El ejecutable no está firmado; si SmartScreen lo bloquea, elige *Más información → Ejecutar de todas
> formas* (o marca **Desbloquear** en *Propiedades* del zip antes de descomprimir).

### Desinstalar

Desde *Configuración → Aplicaciones*, con `winget uninstall sardilac.TaskbarPanels` o (instalación con zip):

```powershell
powershell -ExecutionPolicy Bypass -File "C:\Program Files\TaskbarPanels\uninstall.ps1"
# añade -RemoveSettings para borrar también la configuración
```

La configuración (`%APPDATA%\TaskbarPanels`) se conserva al desinstalar.

## Uso

**Clic derecho** sobre cualquier panel (o sobre su icono en la bandeja) abre el menú:

- **Mostrar IP pública**, **Copiar IP**, **Actualizar IP**, **Actualizar consumo**
- **Abrir uso de Claude / Copilot** (páginas web oficiales)
- **Paneles**: activar o desactivar cada panel
- **Pantallas**: todas, solo la principal o una selección de monitores
- **Estilo del espectro**: onda líquida o barras cápsula
- **IA**: proveedor principal, activar/desactivar proveedores, **agregar** (API de OpenAI, API de Anthropic, personalizado por URL JSON) y quitar
- **Fondo de las ventanas**: vidrio esmerilado, cristal transparente, acrílico de Windows, mica u oscuro
- **Proveedor del speed test**: Speedtest.net, Cloudflare, LibreSpeed o GCI
- **Idioma / Language**: automático (idioma de Windows), español o inglés
- **Reiniciar posición**, **Salir**

Además: los botones multimedia funcionan sin quitar el foco a la ventana activa, el panel de red
se puede **arrastrar** en horizontal y al **dejar el cursor** sobre un panel aparece su ventana de detalle.

## Configuración

Se guarda en `%APPDATA%\TaskbarPanels\config.json` y se aplica en caliente (≤ 5 s):

| Clave | Valores | Descripción |
|---|---|---|
| `Displays` | `"primary"`, `"all"`, `["SAM7B0C", ...]` | Pantallas donde se muestran los paneles (códigos EDID, ver `MonitorIds`) |
| `PanelDisplays` | `{ "MediaPanel": ["..."] }` | Selección por panel: `NetworkPanel`, `UsagePanel`, `SystemPanel`, `MediaPanel` |
| `Panels` | `{ "SystemPanel": false }` | Activar o desactivar paneles |
| `AiProviders` | lista de proveedores | `{ "id", "type": "claude" \| "copilot" \| "openai" \| "anthropic_api" \| "custom", "name", "enabled", "budget" }`; los personalizados añaden `url`, `percentField`, `valueField`, `labelField`. Las claves van al Administrador de credenciales (`TaskbarPanels:<id>`) |
| `AiPrimary` | `"all"` o un `id` | Proveedor que se muestra en la barra |
| `SpeedTestProvider` | `ookla` (predeterminado) / `cloudflare` / `librespeed` / `gci` | Proveedor del speed test de la ventana de red |
| `IpShowAddress` | `true` / `false` | Mostrar la IP pública en el panel de red |
| `SpectrumStyle` | `"wave"`, `"bars"` | Estilo del espectro de audio (panel y ventana) |
| `Language` | `"auto"`, `"es"`, `"en"` | Idioma de la interfaz (por defecto, el de Windows: español o, si no, inglés) |
| `PopupStyle` | `"glass"`, `"clear"`, `"acrylic"`, `"mica"`, `"dark"` | Fondo de las ventanas flotantes |
| `SecondaryRightMargin` | número (px lógicos) | Espacio reservado al reloj en barras secundarias |
| `MonitorIds` | (informativo) | Código → nombre de los monitores detectados |

Otros archivos en la misma carpeta: `pos.json` (separación del panel de red), `ai_cache.json`
(último consumo, evita consultas al reiniciar), `flags\` (caché de banderas), `crash.log` (si el programa falla)
y `debug.log` (solo si existe `debug.flag`).

## Compilar desde el código

Requiere [Rust](https://rustup.rs) (toolchain `stable-x86_64-pc-windows-msvc`) y las
*Build Tools* de Visual Studio con el componente de C++. El instalador necesita además [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```powershell
cargo build --release
.\scripts\install.ps1                 # instala lo recién compilado
.\scripts\make-screenshots.ps1        # regenera docs/images con datos de ejemplo
.\scripts\package.ps1                 # genera dist\TaskbarPanels-vX.Y.Z-x64.zip y el instalador Setup .exe (+ .sha256)
```

Estructura:

```
src/
  main.rs     ventanas, dibujo de los paneles, menús y ventanas flotantes
  render.rs   lienzo con alfa por píxel (UpdateLayeredWindow), formas y texto suavizados
  host.rs     barras de tareas, monitores, orientación, pantalla completa (Win32 + UI Automation)
  state.rs    estado compartido e IP pública
  ai.rs       proveedores de IA (Claude, Copilot, OpenAI, Anthropic, personalizados) y credenciales
  speedtest.rs prueba de velocidad (latencia, bajada y subida): Speedtest.net, Cloudflare, LibreSpeed o GCI
  flags.rs    bandera del país de la IP pública (flagcdn.com, con caché local)
  net.rs      red local: adaptador, puerta de enlace, DNS (GetAdaptersAddresses) y ping (IcmpSendEcho)
  sys.rs      CPU/disco (PDH), GPU (NVML), sensores Lenovo (WMI), FPS (PresentMon), RAM, unidades
  media.rs    sesión multimedia (WinRT), carátula, línea de tiempo y espectro (WASAPI + FFT)
  i18n.rs     idioma de la interfaz (español / inglés según Windows)
  config.rs   config.json
installer/    script de Inno Setup (TaskbarPanels.iss)
scripts/      install / uninstall / installer-helper / package / make-screenshots
```

## Publicar un release

El workflow `.github/workflows/release.yml` compila, empaqueta y publica el release al subir un tag;
después, su job `winget` abre el pull request de actualización en `microsoft/winget-pkgs` (`winget.yml` permite repetirlo a mano).

```powershell
# 1. sube la versión en Cargo.toml y anota los cambios en CHANGELOG.md / CHANGELOG.en.md
git commit -am "v1.0.1"
git tag v1.0.1
git push origin main --tags
```

El release incluye `TaskbarPanels-Setup-vX.Y.Z-x64.exe`, `TaskbarPanels-vX.Y.Z-x64.zip` y sus `.sha256`.
El workflow de winget necesita el secreto `WINGET_TOKEN` (PAT clásico con el permiso `public_repo`)
y un fork de `microsoft/winget-pkgs` en la misma cuenta.

## Privacidad y notas

- Todo se procesa localmente. Las únicas conexiones de red son:
  - `ifconfig.co` (y `api.ipify.org` de respaldo) para la IP pública, cada 5 min;
  - `api.anthropic.com/api/oauth/usage` con el token de Claude Code, cada 3 min;
  - `api.github.com/copilot_internal/user` con tu credencial de GitHub, cada 5 min;
  - si los agregas: `api.openai.com/v1/organization/costs`, `api.anthropic.com/v1/organizations/cost_report` (cada 5 min) y las URL de tus proveedores personalizados (cada 2 min);
  - `api.github.com` / `github.com` solo durante la instalación, para descargar PresentMon;
  - ping ICMP a la **puerta de enlace local**, solo mientras la ventana de red está abierta;
  - `www.cloudflare.com/cdn-cgi/trace` junto con la consulta de IP, para confirmar el país;
  - `flagcdn.com` para la bandera del país de la IP, una sola vez por país (queda en caché);
  - el proveedor de speed test elegido (`speed.cloudflare.com`, servidores de Speedtest.net vía `www.speedtest.net/api/js/servers`, servidores públicos de LibreSpeed vía `librespeed.org/backend-servers` o `speedtest.gci.co.cr`), solo al pulsar **Iniciar prueba** en la ventana de red (hasta ~350 MB).
- Los endpoints de consumo de Claude y Copilot **no están documentados**: pueden cambiar sin aviso.
  Si responden con error o límite de consultas (429), se conservan los últimos valores.
- El programa **no renueva** el token de Claude Code: si caduca, se renueva al volver a abrir Claude Code.

## Solución de problemas

| Problema | Qué hacer |
|---|---|
| No aparecen los paneles | Comprueba la tarea: `Get-ScheduledTask TaskbarPanels`; revisa `%APPDATA%\TaskbarPanels\crash.log` |
| Faltan en una pantalla | Menú **Pantallas**; se ocultan a propósito cuando la barra no se ve (pantalla completa, auto-ocultar) |
| FPS siempre «—» | ¿Existe `C:\Program Files\TaskbarPanels\tools\PresentMon.exe`? Reinstala con la tarea de PresentMon |
| Temperatura de CPU «—» | Solo disponible en portátiles Lenovo compatibles |
| Consumo de Claude «—» | Abre Claude Code para renovar la sesión; espera hasta 3 min |
| Diagnóstico detallado | Crea `%APPDATA%\TaskbarPanels\debug.flag` y revisa `debug.log` |

## Licencia

[MIT](LICENSE). PresentMon es © Intel Corporation (licencia MIT) y se descarga de su repositorio oficial.
