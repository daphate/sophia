$ErrorActionPreference = "Stop"

$Repo = "https://github.com/daphate/sophia.git"
$Dir = "sophia"

Write-Host "=== Sophia Bot Installer ==="

# Check Rust
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Rust not found. Installing via rustup..."
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile "$env:TEMP\rustup-init.exe"
    & "$env:TEMP\rustup-init.exe" -y
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}

# Clone or update
if (Test-Path $Dir) {
    Write-Host "Directory '$Dir' exists, pulling latest..."
    Push-Location $Dir
    git pull
} else {
    Write-Host "Cloning repository..."
    git clone $Repo
    Push-Location $Dir
}

# Build
Write-Host "Building (release)..."
cargo build --release

# Config
if (-not (Test-Path .env)) {
    Copy-Item .env.example .env
    Write-Host ""
    Write-Host "Created .env from template. Edit it with your credentials:"
    Write-Host "  notepad $PWD\.env"
}

Pop-Location

Write-Host ""
Write-Host "Done! Run with:"
Write-Host "  cd $Dir && .\target\release\sophia.exe"
