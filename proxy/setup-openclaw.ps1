# Sophia Proxy — OpenClaw integration setup (Windows, Anthropic Messages API)
# Configures OpenClaw to use sophia-proxy as the default model provider.
# Run as: powershell -ExecutionPolicy Bypass -File setup-openclaw.ps1

$ErrorActionPreference = "Stop"

$ProxyHost = if ($env:SOPHIA_PROXY_HOST) { $env:SOPHIA_PROXY_HOST } else { "127.0.0.1" }
$ProxyPort = if ($env:SOPHIA_PROXY_PORT) { $env:SOPHIA_PROXY_PORT } else { "8080" }
$ProxyUrl = "http://${ProxyHost}:${ProxyPort}/v1"
$ModelId = "secondf8n/sophia"
$ProviderName = "sophia-proxy"

Write-Host "=== Sophia Proxy — OpenClaw setup ===" -ForegroundColor Cyan
Write-Host "Proxy URL: $ProxyUrl"
Write-Host "Model:     $ProviderName/$ModelId"
Write-Host ""

# Check proxy is reachable
try {
    $null = Invoke-RestMethod -Uri "$ProxyUrl/models" -TimeoutSec 5
    Write-Host "Proxy is reachable."
} catch {
    Write-Host "ERROR: Sophia proxy is not reachable at $ProxyUrl" -ForegroundColor Red
    Write-Host "Make sure sophia-proxy is running first."
    exit 1
}

# Find openclaw config
$ConfigPath = if ($env:OPENCLAW_CONFIG) { $env:OPENCLAW_CONFIG } else { "$env:USERPROFILE\.openclaw\openclaw.json" }
if (-not (Test-Path $ConfigPath)) {
    Write-Host "ERROR: OpenClaw config not found at $ConfigPath" -ForegroundColor Red
    Write-Host "Set OPENCLAW_CONFIG env var to specify a different path."
    exit 1
}
Write-Host "OpenClaw config: $ConfigPath"

# Backup
$Backup = "$ConfigPath.backup.$(Get-Date -Format 'yyyyMMddHHmmss')"
Copy-Item $ConfigPath $Backup
Write-Host "Backup: $Backup"

# Read and patch config
$config = Get-Content $ConfigPath -Raw | ConvertFrom-Json

# Ensure models.providers exists
if (-not $config.models) {
    $config | Add-Member -NotePropertyName "models" -NotePropertyValue ([PSCustomObject]@{ providers = [PSCustomObject]@{} })
}
if (-not $config.models.providers) {
    $config.models | Add-Member -NotePropertyName "providers" -NotePropertyValue ([PSCustomObject]@{})
}

# Add sophia-proxy provider
$provider = [PSCustomObject]@{
    baseUrl = $ProxyUrl
    apiKey = "sk-sophia-local"
    api = "anthropic-messages"
    models = @(
        [PSCustomObject]@{
            id = $ModelId
            name = "Sophia (Claude via CLI proxy)"
            contextWindow = 1000000
            maxTokens = 64000
        }
    )
}

$config.models.providers | Add-Member -NotePropertyName $ProviderName -NotePropertyValue $provider -Force

# Set as primary model
if (-not $config.agents) {
    $config | Add-Member -NotePropertyName "agents" -NotePropertyValue ([PSCustomObject]@{ defaults = [PSCustomObject]@{ model = [PSCustomObject]@{} } })
}
if (-not $config.agents.defaults) {
    $config.agents | Add-Member -NotePropertyName "defaults" -NotePropertyValue ([PSCustomObject]@{ model = [PSCustomObject]@{} })
}
if (-not $config.agents.defaults.model) {
    $config.agents.defaults | Add-Member -NotePropertyName "model" -NotePropertyValue ([PSCustomObject]@{})
}

$config.agents.defaults.model | Add-Member -NotePropertyName "primary" -NotePropertyValue "$ProviderName/$ModelId" -Force

# Write config
$config | ConvertTo-Json -Depth 20 | Set-Content $ConfigPath -Encoding UTF8

Write-Host ""
Write-Host "=== Done! ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "OpenClaw is now configured to use sophia-proxy."
Write-Host "Primary model: $ProviderName/$ModelId"
Write-Host ""
Write-Host "Restart OpenClaw to apply."
Write-Host ""
Write-Host "To revert:"
Write-Host "  Copy-Item '$Backup' '$ConfigPath'"
