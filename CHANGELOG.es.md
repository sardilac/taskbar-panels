# Changelog

[English](CHANGELOG.md) · **Español**

## v1.3.2

- **Tamaño del panel multimedia**: nuevo menú *Tamaño del panel multimedia* (compacto 400, mediano 480, grande 560 —predeterminado—, extra grande 680, máximo 800 px lógicos) y clave `MediaWidth` para cualquier ancho de 320 a 1200. El panel se sigue reduciendo si el espacio libre es menor.
- Nuevo icono de **Claude**: la mascota pixelada de Claude Code (terracota) en lugar de la estrella de rayos.
- **Todas las GPU y la NPU** en la ventana del sistema: una tarjeta por GPU (verde NVIDIA, rojo AMD, azul Intel) y la NPU (Intel AI Boost, AMD XDNA, Qualcomm Hexagon) en violeta. **Compatibilidad con AMD Radeon e Intel**: el uso y la memoria salen de los contadores de GPU de Windows (los mismos del Administrador de tareas) y la temperatura, el ventilador y la frecuencia de D3DKMT; NVML sigue añadiendo el consumo en vatios en NVIDIA. El panel de la barra muestra la GPU principal (dedicada, con más VRAM). Nuevo parámetro `--gpu-report <archivo>` que escribe las GPU/NPU detectadas y una lectura, para diagnóstico.
- **Barras verticales** (izquierda/derecha): los paneles de sistema, IA y red van como icono por defecto; elegir *Panel* en *Formato de los paneles* conserva el diseño vertical detallado.
- **Iconos de la barra a la izquierda**: el panel multimedia ya no tapa los iconos (el botón Inicio puede empezar justo en el borde de la barra, ese valor se descartaba y la barra se tomaba como centrada). En modo automático, los paneles de sistema e IA pasan a icono cuando con los paneles completos no cabría el multimedia entero entre los iconos y ellos.
- Los paneles en modo icono ya no se expanden cuando falla una consulta de UI Automation a la barra (p. ej. con su menú contextual abierto): se conserva la última posición conocida de los iconos.
- **Tres estilos de espectro nuevos**: LED segmentado (colores neón como el resto de estilos, con picos retenidos), barras con reflejo (barras sobre una línea base con reflejo difuminado) y línea neón (curva luminosa con relleno degradado). Cinco estilos en total, en el menú *Estilo del espectro* (`SpectrumStyle`: `wave`, `bars`, `led`, `mirror`, `line`).
- **Menos memoria**: la memoria privada en uso en reposo baja de ~31 MB a ~5 MB. La app libera las páginas que solo se tocan al arrancar (NVML, UI Automation, WinRT) 45 s después de iniciar, libera el lienzo de la ventana flotante 15 s después de cerrarla y el de los paneles ocultos, usa un único búfer compartido para el texto en lugar de una copia a tamaño completo por lienzo y guarda en caché las fuentes GDI en lugar de crear una por cada texto.
- **Documentación en inglés como principal**: `README.md` y `CHANGELOG.md` pasan a estar en inglés; las versiones en español son `README.es.md` y `CHANGELOG.es.md`.
- **Formato de los paneles** (menú y clave `PanelStyle`): los paneles de sistema e IA pueden ser automáticos (icono solo si no cabe, como antes), panel completo o icono; el de red, panel o icono (nuevo modo icono con bajada y subida en formato corto sobre la gráfica).
- Nuevo icono de **GitHub Copilot**: casco con degradado azul → morado, gafas de piloto y visor oscuro con ojos cian (en lugar de la cara blanca sobre círculo morado).
- **Los paneles ya no desaparecen con el menú Inicio abierto**: ahora son ventanas hijas de la barra de tareas (antes, ventanas flotantes «siempre encima» que Windows dejaba por debajo de la barra mientras Inicio estaba abierto). Siguen la barra, se recrean solos si Explorer se reinicia y, si Windows no permite integrarlos, vuelven al modo flotante. El ejecutable incluye un manifiesto de aplicación (compatibilidad con Windows 8+), necesario para ventanas hijas con transparencia.

## v1.3.1

- **Copilot**: el token de GitHub se busca en varias fuentes (token guardado a mano, `GH_TOKEN`/`GITHUB_TOKEN`, plugins oficiales de Copilot, GitHub CLI y Git Credential Manager) y se usa el primero con acceso a Copilot. Nuevo menú **IA → Token de GitHub para Copilot…** para guardarlo a mano (Administrador de credenciales). El estado distingue «sin credenciales», «el token no tiene acceso a Copilot» y «sin conexión».

## v1.3.0

- **Instalador** `TaskbarPanels-Setup-vX.Y.Z-x64.exe` (Inno Setup): instala en *Archivos de programa*, descarga y verifica PresentMon (tarea opcional), registra la tarea programada y agrega el desinstalador a *Configuración → Aplicaciones*. Admite instalación silenciosa (`/VERYSILENT`).
- **winget**: disponible como `sardilac.TaskbarPanels`; cada release abre automáticamente la actualización en `microsoft/winget-pkgs`.
- **Documentación en inglés** (`README.en.md`, `CHANGELOG.en.md`).

## v1.2.1

- **Bandera del país** de la IP pública a la izquierda de la IP (ventana de red y fila de IP del panel). Se descarga una vez por país desde `flagcdn.com` y queda en caché en `%APPDATA%\TaskbarPanels\flags`. El país se toma de Cloudflare (`cdn-cgi/trace`) cuando no coincide con el de ifconfig.co, que puede ubicar mal bloques de IP anunciados desde otro país.
- **Speed test con cuatro proveedores**: Speedtest.net (Ookla, predeterminado; elige el servidor cercano con menor latencia), Cloudflare, LibreSpeed (código abierto; servidor público con menor latencia) y GCI (`speedtest.gci.co.cr`, servidor LibreSpeed en Costa Rica). Se cambia con el selector de la tarjeta (clic para pasar al siguiente) o desde el menú *Proveedor del speed test* (clave `SpeedTestProvider`). La tarjeta muestra el servidor usado y, si falla, el motivo.
- Speed test: si el servidor limita las descargas grandes (429 de Cloudflare tras varias pruebas seguidas) se reintenta con bloques más pequeños; la fase de subida ya no se queda unos segundos de más esperando a que cierren las conexiones.
- Los paneles se ocultan cuando la barra de tareas no se ve en pantalla: además de detectar ventanas a pantalla completa en primer plano, se comprueba qué ventana hay sobre varios puntos de la barra. Así un juego a pantalla completa (p. ej. Fortnite) ya no queda con los paneles encima aunque el foco esté en otra pantalla.

## v1.2.0

- **Red · speed test**: nueva tarjeta en la ventana de red con un botón para medir la conexión contra `speed.cloudflare.com`: latencia y jitter, bajada y subida (4 conexiones en paralelo, ~8 s por fase) con medidor circular en vivo, barra de progreso y centro de datos de Cloudflare. Solo se ejecuta al pulsar el botón y consume hasta ~350 MB.
- **Barras verticales**: el panel multimedia se ubica arriba, al inicio de la barra (debajo del botón Inicio si está arriba), y solo baja debajo de los iconos si no cabe.

## v1.1.0

- **IA con varios proveedores**: Claude, GitHub Copilot, API de OpenAI (gasto del mes frente a presupuesto), API de Anthropic (Admin API) y proveedores personalizados (URL JSON). Cada uno con su icono; se elige el **proveedor principal** que se muestra en la barra (o un resumen de todos). Menú *IA*: proveedor principal, activar/desactivar, agregar (formulario) y quitar. Las claves se guardan en el Administrador de credenciales de Windows.
- **Modo icono**: si los iconos de las apps llegan a los paneles, los de sistema e IA se comprimen a un icono (anillo de CPU / icono del proveedor con su valor).
- **Idioma**: la interfaz sigue el idioma de Windows (español si el sistema está en español; inglés en cualquier otro caso), con formatos de números y fechas acordes. Se puede forzar desde el menú *Idioma / Language* o con `"Language": "es" | "en" | "auto"`.
- **Multimedia**: el panel solo se muestra si hay una sesión multimedia (reproduciendo o en pausa), con 3 s de gracia; es más compacto (560 px lógicos) y respeta un margen de 16 px con los iconos de la barra, que ahora se ubican con UI Automation desde un hilo en segundo plano.
- **Fondo de las ventanas** configurable: vidrio esmerilado, cristal transparente, acrílico de Windows, mica u oscuro (menú *Fondo de las ventanas*, clave `PopupStyle`).
- **Red**: nueva tarjeta con un router animado, puerta de enlace, DNS, adaptador y ping a la puerta de enlace con gráfica y estadísticas; el ping solo corre mientras la ventana está abierta.
- La ventana multimedia usa el estilo de espectro elegido (onda o barras).
- Los paneles ya no se ocultan por ventanas del shell (desplegables de la bandeja, menú Inicio, notificaciones) ni por ventanas ocultas por DWM; además, ocultar exige que la condición se mantenga ~0,8 s.
- Los paneles ya no desaparecen ~1 s al hacer clic en la barra de tareas (p. ej. en *Mostrar iconos ocultos*): se reafirman por encima de la barra en cuanto cambia la ventana en primer plano.
- Instalador: espera a que se cierre la versión anterior y reintenta la copia (antes podía quedar sin iniciar al actualizar).

## v1.0.0

Primera versión publicada.

- **Red**: velocidad de subida/bajada con gráficas de 60 s, IP pública con indicador de estado y aviso de cambio de IP; opción para ocultar la IP.
- **Consumo de IA**: uso de Claude (sesión de 5 h y semanal) y de GitHub Copilot (solicitudes premium); mostrar uno u otro o ambos.
- **Sistema**: CPU, GPU, temperaturas, FPS (PresentMon) y discos (lectura/escritura y anillo del disco del sistema).
- **Multimedia**: carátula con vinilo animado, botones, título desplazable, progreso y espectro de audio (onda líquida o barras cápsula).
- Ventanas flotantes de vidrio esmerilado con el detalle de cada panel.
- Varias pantallas (todas, solo la principal o una selección), barras pequeñas, arriba o verticales, iconos a la izquierda o centrados.
- Se ocultan con aplicaciones a pantalla completa.
