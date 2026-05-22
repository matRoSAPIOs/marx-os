# scripts/build.ps1
# Сборка MarX-OS: сначала kernel (для x86_64-unknown-none), потом runner (хост).
param(
    [switch]$Release,
    [switch]$Run,
    [switch]$Uefi,
    [switch]$Headless
)

# НЕ используем $ErrorActionPreference='Stop' — в PS5.1 cargo пишет
# нормальную диагностику в stderr, что трактуется как error и рушит скрипт.
# Проверяем только $LASTEXITCODE после нативных вызовов.

# Гарантируем что rustup/cargo и qemu видимы в этой сессии
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files\qemu;$env:PATH"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$profileArg = if ($Release) { '--release' } else { '' }
$profileDir = if ($Release) { 'release'  } else { 'debug' }

Write-Host '=== [1/3] Building kernel (x86_64-unknown-none) ===' -ForegroundColor Cyan
$kernelArgs = @('build', '-p', 'marx-kernel', '--target', 'x86_64-unknown-none')
if ($Release) { $kernelArgs += '--release' }
& cargo @kernelArgs
if ($LASTEXITCODE -ne 0) { throw "kernel build failed" }

Write-Host ''
Write-Host '=== [2/3] Building apps (x86_64-unknown-none, profile=release-app) ===' -ForegroundColor Cyan
# Discover every crate under apps/ and build it with the size-optimised
# release-app profile.  Each crate compiles to target/x86_64-unknown-none/
# release-app/<name>; runner/build.rs picks them up and adds them to MARXARCH.
$appsDir = Join-Path $root 'apps'
if (Test-Path $appsDir) {
    Get-ChildItem $appsDir -Directory | ForEach-Object {
        $appName = $_.Name
        Write-Host "  -> building app '$appName'"
        & cargo build -p $appName --target x86_64-unknown-none --profile release-app
        if ($LASTEXITCODE -ne 0) { throw "app '$appName' build failed" }
    }
}

Write-Host ''
Write-Host '=== [3/3] Building runner (host) ===' -ForegroundColor Cyan
$runnerArgs = @('build', '-p', 'marx-runner')
if ($Release) { $runnerArgs += '--release' }
& cargo @runnerArgs
if ($LASTEXITCODE -ne 0) { throw "runner build failed" }

$runnerExe = "C:\marx-build\target\$profileDir\marx-runner.exe"
Write-Host ''
Write-Host "Runner binary: $runnerExe" -ForegroundColor Green
Write-Host "Disk images:   C:\marx-build\target\$profileDir\build\marx-runner-*\out\marx-bios.img" -ForegroundColor Green

if ($Run) {
    $runnerFlags = @()
    if ($Uefi)     { $runnerFlags += '--uefi' }
    if ($Headless) { $runnerFlags += '--headless' }
    Write-Host ''
    Write-Host "=== Launching QEMU ($($runnerFlags -join ' ')) ===" -ForegroundColor Cyan
    & $runnerExe @runnerFlags
}
