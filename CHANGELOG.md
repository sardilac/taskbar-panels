# Changelog

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
