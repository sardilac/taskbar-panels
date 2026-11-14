# Taskbar Panels

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
| **Red** | Subida y bajada con gráficas de 60 s, IP pública con indicador de estado | ISP, ubicación, router con puerta de enlace, DNS, adaptador y **ping a la puerta de enlace** (solo mientras la ventana está abierta), máximos del último minuto y totales de la sesión |

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
iconos centrados o a la izquierda y escala por monitor. Los paneles se ocultan con juegos o
videos a pantalla completa.

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

1. Descarga `TaskbarPanels-vX.Y.Z-x64.zip` desde [Releases](../../releases) y descomprímelo.
2. Abre PowerShell en esa carpeta y ejecuta:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\install.ps1
   ```

   Se abrirá el aviso de UAC. El instalador:
   - copia el programa a `C:\Program Files\TaskbarPanels`;
   - descarga PresentMon del release oficial de Intel y **verifica su firma digital**
     (usa `-SkipPresentMon` para omitirlo);
   - registra la tarea programada **TaskbarPanels** (al iniciar sesión, se reinicia sola si falla)
     y arranca los paneles.

> El ejecutable no está firmado; si SmartScreen lo bloquea, en *Propiedades* del zip marca
> **Desbloquear** antes de descomprimir.

### Desinstalar

```powershell
powershell -ExecutionPolicy Bypass -File "C:\Program Files\TaskbarPanels\uninstall.ps1"
# añade -RemoveSettings para borrar también la configuración
```

## Uso

**Clic derecho** sobre cualquier panel (o sobre su icono en la bandeja) abre el menú:

- **Mostrar IP pública**, **Copiar IP**, **Actualizar IP**, **Actualizar consumo**
- **Abrir uso de Claude / Copilot** (páginas web oficiales)
- **Paneles**: activar o desactivar cada panel
- **Pantallas**: todas, solo la principal o una selección de monitores
- **Estilo del espectro**: onda líquida o barras cápsula
- **IA**: proveedor principal, activar/desactivar proveedores, **agregar** (API de OpenAI, API de Anthropic, personalizado por URL JSON) y quitar
- **Fondo de las ventanas**: vidrio esmerilado, cristal transparente, acrílico de Windows, mica u oscuro
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
| `IpShowAddress` | `true` / `false` | Mostrar la IP pública en el panel de red |
| `SpectrumStyle` | `"wave"`, `"bars"` | Estilo del espectro de audio (panel y ventana) |
| `Language` | `"auto"`, `"es"`, `"en"` | Idioma de la interfaz (por defecto, el de Windows: español o, si no, inglés) |
| `PopupStyle` | `"glass"`, `"clear"`, `"acrylic"`, `"mica"`, `"dark"` | Fondo de las ventanas flotantes |
| `SecondaryRightMargin` | número (px lógicos) | Espacio reservado al reloj en barras secundarias |
| `MonitorIds` | (informativo) | Código → nombre de los monitores detectados |

Otros archivos en la misma carpeta: `pos.json` (separación del panel de red), `usage_cache.json`
(último consumo, evita consultas al reiniciar), `crash.log` (si el programa falla) y `debug.log`
(solo si existe `debug.flag`).

## Compilar desde el código

Requiere [Rust](https://rustup.rs) (toolchain `stable-x86_64-pc-windows-msvc`) y las
*Build Tools* de Visual Studio con el componente de C++.

```powershell
cargo build --release
.\scripts\install.ps1                 # instala lo recién compilado
.\scripts\make-screenshots.ps1        # regenera docs/images con datos de ejemplo
.\scripts\package.ps1                 # genera dist\TaskbarPanels-vX.Y.Z-x64.zip (+ .sha256)
```

Estructura:

```
src/
  main.rs     ventanas, dibujo de los paneles, menús y ventanas flotantes
  render.rs   lienzo con alfa por píxel (UpdateLayeredWindow), formas y texto suavizados
  host.rs     barras de tareas, monitores, orientación, pantalla completa (Win32 + UI Automation)
  state.rs    estado compartido e IP pública
  ai.rs       proveedores de IA (Claude, Copilot, OpenAI, Anthropic, personalizados) y credenciales
  net.rs      red local: adaptador, puerta de enlace, DNS (GetAdaptersAddresses) y ping (IcmpSendEcho)
  sys.rs      CPU/disco (PDH), GPU (NVML), sensores Lenovo (WMI), FPS (PresentMon), RAM, unidades
  media.rs    sesión multimedia (WinRT), carátula, línea de tiempo y espectro (WASAPI + FFT)
  i18n.rs     idioma de la interfaz (español / inglés según Windows)
  config.rs   config.json
scripts/      install / uninstall / package / make-screenshots
```

## Publicar un release

El workflow `.github/workflows/release.yml` compila, empaqueta y publica el release al subir un tag:

```powershell
# 1. sube la versión en Cargo.toml y anota los cambios en CHANGELOG.md
git commit -am "v1.0.1"
git tag v1.0.1
git push origin main --tags
```

El release incluye `TaskbarPanels-vX.Y.Z-x64.zip` y su `.sha256`.

## Privacidad y notas

- Todo se procesa localmente. Las únicas conexiones de red son:
  - `ifconfig.co` (y `api.ipify.org` de respaldo) para la IP pública, cada 5 min;
  - `api.anthropic.com/api/oauth/usage` con el token de Claude Code, cada 3 min;
  - `api.github.com/copilot_internal/user` con tu credencial de GitHub, cada 5 min;
  - si los agregas: `api.openai.com/v1/organization/costs`, `api.anthropic.com/v1/organizations/cost_report` (cada 5 min) y las URL de tus proveedores personalizados (cada 2 min);
  - `api.github.com` / `github.com` solo durante la instalación, para descargar PresentMon;
  - ping ICMP a la **puerta de enlace local**, solo mientras la ventana de red está abierta.
- Los endpoints de consumo de Claude y Copilot **no están documentados**: pueden cambiar sin aviso.
  Si responden con error o límite de consultas (429), se conservan los últimos valores.
- El programa **no renueva** el token de Claude Code: si caduca, se renueva al volver a abrir Claude Code.

## Solución de problemas

| Problema | Qué hacer |
|---|---|
| No aparecen los paneles | Comprueba la tarea: `Get-ScheduledTask TaskbarPanels`; revisa `%APPDATA%\TaskbarPanels\crash.log` |
| Faltan en una pantalla | Menú **Pantallas**; en pantalla completa o con la barra auto-ocultada se ocultan a propósito |
| FPS siempre «—» | ¿Existe `C:\Program Files\TaskbarPanels\tools\PresentMon.exe`? Reinstala sin `-SkipPresentMon` |
| Temperatura de CPU «—» | Solo disponible en portátiles Lenovo compatibles |
| Consumo de Claude «—» | Abre Claude Code para renovar la sesión; espera hasta 3 min |
| Diagnóstico detallado | Crea `%APPDATA%\TaskbarPanels\debug.flag` y revisa `debug.log` |

## Licencia

[MIT](LICENSE). PresentMon es © Intel Corporation (licencia MIT) y se descarga de su repositorio oficial.
