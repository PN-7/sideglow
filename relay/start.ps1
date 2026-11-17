param([string]$ConfigPath = (Join-Path $PSScriptRoot 'config.json'))

$ErrorActionPreference = 'Stop'
$createdVariables = @()

function Read-MaskedValue([string]$Prompt) {
    $secure = Read-Host -Prompt $Prompt -AsSecureString
    $pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try { return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer) }
}

try {
    $absoluteConfig = (Resolve-Path -LiteralPath $ConfigPath).Path
    $config = Get-Content -LiteralPath $absoluteConfig -Raw -Encoding UTF8 | ConvertFrom-Json
    $botVariable = if ($config.botTokenEnv) { $config.botTokenEnv } else { 'DISCORD_BOT_TOKEN' }
    $webhookVariable = if ($config.webhookUrlEnv) { $config.webhookUrlEnv } else { 'DISCORD_WEBHOOK_URL' }
    $variables = @($botVariable, $webhookVariable) + @($config.users | ForEach-Object { $_.accessKeyEnv })
    foreach ($name in ($variables | Select-Object -Unique)) {
        if ($name -notmatch '^[A-Za-z_][A-Za-z0-9_]*$') { throw 'Invalid environment variable name in configuration.' }
        if (-not [Environment]::GetEnvironmentVariable($name, 'Process')) {
            $value = Read-MaskedValue "Enter $name (hidden input)"
            [Environment]::SetEnvironmentVariable($name, $value, 'Process')
            $createdVariables += $name
            $value = $null
        }
    }
    & node (Join-Path $PSScriptRoot 'main.mjs') --config $absoluteConfig
    $result = $LASTEXITCODE
} finally {
    foreach ($name in $createdVariables) { [Environment]::SetEnvironmentVariable($name, $null, 'Process') }
}
if ($null -ne $result) { exit $result }
