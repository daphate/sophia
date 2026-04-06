# Sophia Proxy — Windows installer (PowerShell)
# Run as: irm https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-windows.ps1 | iex

$ErrorActionPreference = "Stop"

$RepoUrl = "https://github.com/daphate/sophia-proxy.git"
$InstallDir = if ($env:SOPHIA_INSTALL_DIR) { $env:SOPHIA_INSTALL_DIR } else { "$env:USERPROFILE\sophia-proxy" }

Write-Host "=== Sophia Proxy — Windows installer ===" -ForegroundColor Cyan

# Check Rust
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Rust not found. Please install from https://rustup.rs" -ForegroundColor Red
    Write-Host "After installing, restart this script."
    exit 1
}

# Check Claude CLI
if (-not (Get-Command claude -ErrorAction SilentlyContinue)) {
    Write-Host "Claude CLI not found."
    if (Get-Command npm -ErrorAction SilentlyContinue) {
        Write-Host "Installing via npm..."
        npm install -g @anthropic-ai/claude-code
    } else {
        Write-Host "ERROR: Install Claude CLI: npm install -g @anthropic-ai/claude-code" -ForegroundColor Red
        exit 1
    }
}

# Clone or update repo
$ProxyDir = Join-Path $InstallDir "proxy"
if (Test-Path (Join-Path $ProxyDir "src")) {
    Write-Host "Updating existing installation at $InstallDir..."
    git -C $InstallDir pull --ff-only
} else {
    Write-Host "Cloning repository to $InstallDir..."
    git clone $RepoUrl $InstallDir
}

# Build
Set-Location $ProxyDir
Write-Host "Building sophia-proxy..."
cargo build --release
$Binary = Join-Path $ProxyDir "target\release\sophia-proxy.exe"
Write-Host "Built: $Binary"

# Create .env if missing
$EnvFile = Join-Path $ProxyDir ".env"
$ClaudePath = if (Get-Command claude -ErrorAction SilentlyContinue) { (Get-Command claude).Source } else { "claude" }
if (-not (Test-Path $EnvFile)) {
    @"
PROXY_HOST=127.0.0.1
PROXY_PORT=8080
CLAUDE_CLI=$ClaudePath
MODEL_NAME=claude-opus-4-6
INFERENCE_TIMEOUT=300
"@ | Set-Content $EnvFile -Encoding UTF8
    Write-Host "Created .env (CLAUDE_CLI=$ClaudePath). Edit as needed."
}

# Register as Windows Service using sc.exe (requires Admin)
$ServiceName = "SophiaProxy"
$IsAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if ($IsAdmin) {
    sc.exe stop $ServiceName 2>$null
    sc.exe delete $ServiceName 2>$null
    Start-Sleep -Seconds 1

    $WrapperScript = Join-Path $ProxyDir "run-proxy.cmd"
    @"
@echo off
cd /d "$ProxyDir"
"$Binary"
"@ | Set-Content $WrapperScript -Encoding ASCII

    sc.exe create $ServiceName binPath= "`"$WrapperScript`"" start= auto DisplayName= "Sophia Proxy"
    sc.exe description $ServiceName "Anthropic-compatible proxy for Claude CLI"
    sc.exe start $ServiceName

    Write-Host ""
    Write-Host "Service '$ServiceName' installed and started." -ForegroundColor Green
    Write-Host ""
    Write-Host "Commands:"
    Write-Host "  Stop:    sc.exe stop $ServiceName"
    Write-Host "  Start:   sc.exe start $ServiceName"
    Write-Host "  Remove:  sc.exe delete $ServiceName"
} else {
    Write-Host ""
    Write-Host "Not running as Admin — skipping service registration." -ForegroundColor Yellow
    Write-Host "To run manually:"
    Write-Host "  cd $ProxyDir"
    Write-Host "  .\target\release\sophia-proxy.exe"
    Write-Host ""
    Write-Host "To install as service, re-run this script as Administrator."
}

Write-Host ""
Write-Host "=== Done! ===" -ForegroundColor Cyan
Write-Host "Sophia proxy: http://127.0.0.1:8080"
Write-Host "Test: curl http://127.0.0.1:8080/v1/models"
