# Changelog

[English](CHANGELOG.md) · **Español**

## Sin publicar

- **Panel de radio** (nuevo): emisoras por Internet de todos los países de América y Europa (directorio Radio Browser; el catálogo de cada país se guarda y se renueva a diario). En la barra muestra el letrero **ON AIR** con el nombre de la emisora mientras suena y, en modo icono, una nota musical roja. La ventana flotante permite elegir primero el país y luego la emisora, buscar por nombre, ciudad o género (también en el directorio completo del país), detener o reanudar, ajustar el volumen y ver los datos de la emisora actual. La emisora también aparece en los controles multimedia de Windows (y en el panel multimedia, con su espectro). Se activa o desactiva en *Paneles → Radio* y su formato (automático, panel o icono) en *Formato de los paneles*. Diagnóstico: `taskbar-panels.exe --radio-report <archivo>`.

## v1.4.0

- **Salud de la conexión** (panel de red): cada 2 s se hace ping al router, al servidor DNS y a Internet, y se comparan tramo a tramo para decir dónde está el problema: *Wi-Fi inestable*, *El router no responde*, *Sin acceso a Internet*, *Internet con pérdidas*, *Internet con latencia alta* o *DNS lento*. El panel muestra un punto de estado (verde, ámbar o rojo con halo); la ventana flotante añade una tarjeta con el diagnóstico y una línea de tiempo de 2 minutos de latencia y pérdidas por tramo. Los destinos que nunca responden a ping se excluyen en vez de dar falsas alarmas. Se puede desactivar desde el menú (*Salud de la conexión*) o con `"NetHealth": false`.
- **Más proveedores de IA** (menú *IA → Agregar proveedor*):
  - **DeepSeek** y **Kimi (Moonshot AI)**: saldo de la cuenta; con un saldo de referencia opcional (tu última recarga) la barra indica cuánto se ha consumido. Kimi admite el servidor de la plataforma china (`https://api.moonshot.cn`, saldo en yuanes).
  - **Ollama (local)** y **LM Studio**: modelos cargados en memoria frente a los instalados, con sus nombres (y la memoria en uso en Ollama). Se consulta cada 30 s; la tarjeta avisa si el servidor no está en marcha. La dirección del servidor se puede cambiar para llegar a otro equipo.
  - **Gemini** y **Ollama Cloud**: ninguno publica el consumo ni la cuota restante, así que se comprueba la clave y se cuentan los modelos disponibles.
  - Iconos dibujados nuevos para cada uno. Los indicadores sin porcentaje (saldo, número de modelos) usan el color del proveedor y no muestran una barra vacía.
- **Ventiladores y energía** (ventana de sistema):
  - **Ventiladores**: una tarjeta con un rotor animado para el ventilador de la CPU y el de la GPU que gira en proporción a las rpm reales, junto a su valor. Solo aparece en equipos que exponen la velocidad de los ventiladores.
  - **Batería**: nivel con color según la carga, un brillo que la recorre mientras carga, potencia de carga o descarga en vatios y autonomía estimada. En sobremesa se indica *Sin batería*.
  - **Plan de energía**: nombre del plan activo y, en Windows 11, el modo de energía (*Mejor eficiencia* / *Mejor rendimiento*), con flechas que avanzan despacio en verde en ahorro, en azul en equilibrado y rápido en naranja en alto rendimiento.
- **Punto de estado del sistema** (panel de sistema): verde si todo va bien, ámbar o rojo con halo cuando algo merece atención, como el de la red. Vigila la temperatura de la CPU y de la GPU (solo si se mantiene alta unos 30 s, no por un pico), la memoria en uso (90 % / 96 %), el espacio libre del disco del sistema (10 % / 5 %) y el nivel de batería sin enchufar (15 % / 7 %). La ventana flotante enumera arriba los avisos activos.
- **Lo que más consume** (ventana de sistema): las cinco aplicaciones que más CPU y memoria usan en ese momento, con los procesos del mismo programa sumados. Solo se muestrea mientras la ventana está abierta.
- **Ventana de sistema en dos columnas**: CPU, GPU, NPU y ventiladores a la izquierda; memoria, discos, energía, aplicaciones y FPS a la derecha. Las dos columnas terminan siempre a la misma altura.
- **Menos consumo de CPU y memoria**:
  - Las ventanas flotantes repintan solo sus partes animadas (router; ventiladores y energía) entre los repintados completos de cada segundo, en lugar de toda la ventana en cada fotograma.
  - El panel multimedia se dibuja a 30 fps mientras reproduce (el espectro se analiza a 30 Hz) y a 10 fps en pausa, y no se dibuja si no hay ningún panel multimedia visible. El suavizado del espectro ya no depende de los fotogramas por segundo.
  - El color de las barras se lee de la pantalla solo durante 20 s tras un cambio de modo de Windows o al aparecer una barra, y cada 30 s el resto del tiempo, en lugar de cada segundo.
  - El dibujo de texto salta los píxeles vacíos y usa una tabla de cobertura precalculada; el fondo de las ventanas ya no se duplica en memoria en cada repintado.
  - El observador de iconos de la barra consulta cada 1,5 s con la barra estable (0,7 s durante 10 s tras un cambio), y el texto del icono de la bandeja solo se actualiza cuando cambia.
  - Con `debug.flag`, `debug.log` registra cada 10 s el tiempo de CPU por hilo y por sección del hilo de interfaz.

## v1.3.4

- **Modo claro y oscuro**: los paneles y las ventanas flotantes siguen el modo de Windows y cambian al instante. En modo claro cada color se convierte en su equivalente oscuro (se invierte la luminosidad perceptual OKLab conservando el tono), así textos, gráficas e indicadores se leen bien sobre una barra clara; las ventanas flotantes usan tinte claro y el modo claro de DWM. Los iconos de proveedores, la carátula y el vinilo conservan sus colores.
- **Cada barra se evalúa por su color real en pantalla**: Explorer a veces deja una barra (normalmente una secundaria) oscura tras cambiar de modo; los paneles de esa barra mantienen la paleta oscura en vez de quedar ilegibles.
- El fondo *Oscuro* de las ventanas pasa a llamarse *Opaco* y sigue el modo (`PopupStyle` sigue siendo `"dark"`).
- El botón del speed test usa azul de acento con texto blanco en modo claro.
- `--render-demo` genera también las imágenes en modo claro (`*-light.png`).

## v1.3.3

- **Paneles de red, IA y sistema arrastrables por separado**: cada uno guarda su propia posición en la barra (antes solo se arrastraba el de red y movía los tres juntos). Si un panel llega a otro lo empuja, sin solaparse nunca; al pasar por encima de un vecino intercambian el orden. Ninguno sale de la barra ni invade el espacio del panel multimedia, que se aparta o se oculta si no cabe. `pos.json` guarda ahora `Offsets` por panel (el `Gap` anterior se migra solo) y *Reiniciar posición* los vuelve a juntar a la derecha.
- **Panel de sistema más estrecho** (236 → 150 px lógicos): gráficas de CPU/GPU/FPS más compactas y sin el texto de espacio libre y lectura/escritura del disco; queda el anillo con el % usado (el detalle sigue en la ventana flotante).
- **Panel de red más estrecho** (112 → 96 px lógicos); una IP que no quepa se corta con «…».

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
