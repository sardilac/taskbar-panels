<#
  Formulario para agregar un proveedor de IA a Taskbar Panels.
  Lo lanza taskbar-panels.exe; escribe en la salida estándar un JSON con los datos
  (o nada si se cancela). Tipos: openai, anthropic_api, gemini, deepseek, kimi, ollama,
  ollama_cloud, lmstudio, custom, copilot (solo el token).
#>
param([string]$Kind = 'openai', [string]$Lang = 'es')

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
$es = $Lang -eq 'es'
function T($a, $b) { if ($es) { $a } else { $b } }

$titles = @{
    openai        = (T 'Agregar API de OpenAI' 'Add OpenAI API')
    anthropic_api = (T 'Agregar API de Anthropic' 'Add Anthropic API')
    custom        = (T 'Agregar proveedor personalizado' 'Add custom provider')
    gemini        = (T 'Agregar Gemini' 'Add Gemini')
    deepseek      = (T 'Agregar DeepSeek' 'Add DeepSeek')
    kimi          = (T 'Agregar Kimi (Moonshot AI)' 'Add Kimi (Moonshot AI)')
    ollama        = (T 'Agregar Ollama (local)' 'Add Ollama (local)')
    ollama_cloud  = (T 'Agregar Ollama Cloud' 'Add Ollama Cloud')
    lmstudio      = (T 'Agregar LM Studio' 'Add LM Studio')
    copilot       = (T 'Token de GitHub para Copilot' 'GitHub token for Copilot')
}
$notes = @{
    openai        = (T 'Usa una clave de administrador (Admin key) de platform.openai.com. Se mide el gasto del mes frente al presupuesto.' 'Use an Admin key from platform.openai.com. Shows this month''s spend against your budget.')
    anthropic_api = (T 'Usa una clave de la Admin API de console.anthropic.com. Se mide el gasto del mes frente al presupuesto.' 'Use an Admin API key from console.anthropic.com. Shows this month''s spend against your budget.')
    gemini        = (T 'Usa una clave de API de aistudio.google.com. La API de Gemini no publica el consumo ni la cuota restante: se comprueba la clave y se muestran los modelos a los que da acceso.' 'Use an API key from aistudio.google.com. The Gemini API does not publish usage or remaining quota: the key is checked and the models it gives access to are shown.')
    deepseek      = (T 'Usa una clave de API de platform.deepseek.com. Se muestra el saldo de la cuenta; con un saldo de referencia (tu última recarga) la barra indica cuánto has consumido.' 'Use an API key from platform.deepseek.com. Shows the account balance; with a reference balance (your last top-up) the bar shows how much you have used.')
    kimi          = (T 'Usa una clave de API de platform.moonshot.ai. Se muestra el saldo disponible. Las cuentas de la plataforma china usan https://api.moonshot.cn (saldo en yuanes).' 'Use an API key from platform.moonshot.ai. Shows the available balance. Accounts on the Chinese platform use https://api.moonshot.cn (balance in yuan).')
    ollama        = (T 'Servidor de Ollama en este equipo o en la red. Se muestran los modelos cargados en memoria y los instalados. No tiene cuota ni necesita clave.' 'Ollama server on this PC or on the network. Shows the models loaded in memory and the installed ones. It has no quota and needs no key.')
    ollama_cloud  = (T 'Usa una clave de API de ollama.com. Ollama no publica el consumo de la cuenta: se comprueba la clave y se muestran los modelos disponibles en la nube.' 'Use an API key from ollama.com. Ollama does not publish account usage: the key is checked and the models available in the cloud are shown.')
    lmstudio      = (T 'Servidor local de LM Studio (pestaña Developer, Start Server). Se muestran los modelos cargados y los descargados. La clave solo hace falta si activaste la autenticación.' 'LM Studio local server (Developer tab, Start Server). Shows the loaded and the downloaded models. The key is only needed if you turned authentication on.')
    custom        = (T 'URL que devuelve un JSON. Indica la ruta del campo con el porcentaje (p. ej. usage.percent).' 'A URL returning JSON. Set the path of the percentage field (e.g. usage.percent).')
    copilot       = (T 'Solo hace falta si no se detecta solo (Git Credential Manager, GitHub CLI o plugins de Copilot). Usa el token de "gh auth token" de la cuenta con Copilot. Se guarda en el Administrador de credenciales; deja el campo vacío para borrarlo.' 'Only needed if it is not detected automatically (Git Credential Manager, GitHub CLI or Copilot plugins). Use the "gh auth token" token of the account with Copilot. It is stored in Windows Credential Manager; leave it empty to remove it.')
}

$form = New-Object System.Windows.Forms.Form
$form.Text = $titles[$Kind]
$form.Font = New-Object System.Drawing.Font('Segoe UI', 10)
$form.FormBorderStyle = 'FixedDialog'; $form.MaximizeBox = $false; $form.MinimizeBox = $false
$form.StartPosition = 'CenterScreen'; $form.TopMost = $true
$form.AutoScaleMode = 'Dpi'; $form.Padding = New-Object System.Windows.Forms.Padding(16)
$form.AutoSize = $true; $form.AutoSizeMode = 'GrowAndShrink'

$panel = New-Object System.Windows.Forms.TableLayoutPanel
$panel.ColumnCount = 2; $panel.AutoSize = $true; $panel.Dock = 'Fill'
$form.Controls.Add($panel)

$note = New-Object System.Windows.Forms.Label
$note.Text = $notes[$Kind]; $note.MaximumSize = New-Object System.Drawing.Size(460, 0); $note.AutoSize = $true
$note.ForeColor = [System.Drawing.Color]::DimGray; $note.Margin = New-Object System.Windows.Forms.Padding(0, 0, 0, 12)
$panel.Controls.Add($note); $panel.SetColumnSpan($note, 2)

$fields = @{}
function Add-Field($key, $label, $default = '', [switch]$Secret) {
    $l = New-Object System.Windows.Forms.Label
    $l.Text = $label; $l.AutoSize = $true; $l.Anchor = 'Left'; $l.Margin = New-Object System.Windows.Forms.Padding(0, 6, 12, 6)
    $t = New-Object System.Windows.Forms.TextBox
    $t.Width = 300; $t.Text = $default
    if ($Secret) { $t.UseSystemPasswordChar = $true }
    $panel.Controls.Add($l); $panel.Controls.Add($t)
    $script:fields[$key] = $t
}

switch ($Kind) {
    'copilot' {
        Add-Field 'key' (T 'Token de GitHub' 'GitHub token') -Secret
    }
    'openai' {
        Add-Field 'name' (T 'Nombre' 'Name') 'OpenAI API'
        Add-Field 'key' (T 'Clave de administrador' 'Admin key') -Secret
        Add-Field 'budget' (T 'Presupuesto mensual (USD)' 'Monthly budget (USD)') '50'
    }
    'anthropic_api' {
        Add-Field 'name' (T 'Nombre' 'Name') 'Anthropic API'
        Add-Field 'key' (T 'Clave de la Admin API' 'Admin API key') -Secret
        Add-Field 'budget' (T 'Presupuesto mensual (USD)' 'Monthly budget (USD)') '100'
    }
    'gemini' {
        Add-Field 'name' (T 'Nombre' 'Name') 'Gemini'
        Add-Field 'key' (T 'Clave de API' 'API key') -Secret
    }
    'deepseek' {
        Add-Field 'name' (T 'Nombre' 'Name') 'DeepSeek'
        Add-Field 'key' (T 'Clave de API' 'API key') -Secret
        Add-Field 'budget' (T 'Saldo de referencia (opcional)' 'Reference balance (optional)') ''
    }
    'kimi' {
        Add-Field 'name' (T 'Nombre' 'Name') 'Kimi'
        Add-Field 'key' (T 'Clave de API' 'API key') -Secret
        Add-Field 'url' (T 'Servidor' 'Server') 'https://api.moonshot.ai'
        Add-Field 'budget' (T 'Saldo de referencia (opcional)' 'Reference balance (optional)') ''
    }
    'ollama' {
        Add-Field 'name' (T 'Nombre' 'Name') 'Ollama'
        Add-Field 'url' (T 'Servidor' 'Server') 'http://localhost:11434'
    }
    'ollama_cloud' {
        Add-Field 'name' (T 'Nombre' 'Name') 'Ollama Cloud'
        Add-Field 'key' (T 'Clave de API' 'API key') -Secret
    }
    'lmstudio' {
        Add-Field 'name' (T 'Nombre' 'Name') 'LM Studio'
        Add-Field 'url' (T 'Servidor' 'Server') 'http://localhost:1234'
        Add-Field 'key' (T 'Clave de API (opcional)' 'API key (optional)') -Secret
    }
    default {
        Add-Field 'name' (T 'Nombre' 'Name') ''
        Add-Field 'url' 'URL' 'https://'
        Add-Field 'percentField' (T 'Campo del porcentaje' 'Percentage field') 'usage.percent'
        Add-Field 'valueField' (T 'Campo del valor (opcional)' 'Value field (optional)') ''
        Add-Field 'key' (T 'Token Bearer (opcional)' 'Bearer token (optional)') -Secret
    }
}

$buttons = New-Object System.Windows.Forms.FlowLayoutPanel
$buttons.FlowDirection = 'RightToLeft'; $buttons.AutoSize = $true; $buttons.Dock = 'Fill'
$buttons.Margin = New-Object System.Windows.Forms.Padding(0, 14, 0, 0)
$ok = New-Object System.Windows.Forms.Button; $ok.Text = if ($Kind -eq 'copilot') { (T 'Guardar' 'Save') } else { (T 'Agregar' 'Add') }; $ok.AutoSize = $true; $ok.DialogResult = 'OK'
$cancel = New-Object System.Windows.Forms.Button; $cancel.Text = (T 'Cancelar' 'Cancel'); $cancel.AutoSize = $true; $cancel.DialogResult = 'Cancel'
$buttons.Controls.Add($ok); $buttons.Controls.Add($cancel)
$panel.Controls.Add($buttons); $panel.SetColumnSpan($buttons, 2)
$form.AcceptButton = $ok; $form.CancelButton = $cancel

if ($form.ShowDialog() -eq 'OK') {
    $out = @{}
    foreach ($k in $fields.Keys) { $out[$k] = $fields[$k].Text.Trim() }
    if (-not $out['name']) { $out['name'] = $titles[$Kind] }
    [Console]::Out.WriteLine(($out | ConvertTo-Json -Compress))
}
