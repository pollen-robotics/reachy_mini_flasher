<#
.SYNOPSIS
    Windows counterpart of fetch-rpiboot.sh: stage the rpiboot artifacts the
    flasher bundles as Tauri resources.

.DESCRIPTION
    On macOS/Linux rpiboot is built from source. On Windows there is no build
    step worth doing: rpiboot.exe is a Cygwin binary that Raspberry Pi ships
    prebuilt, and it only exists inside their installer (it is NOT checked into
    raspberrypi/usbboot). So this downloads rpiboot_setup.exe, unpacks it with
    7-Zip, and copies out what we need:

        rpiboot.exe               the loader itself
        cygusb-1.0.dll            \ Cygwin runtime - rpiboot.exe will not
        cygwin1.dll               / start without both of these next to it
        mass-storage-gadget64\    boot files, incl. the REAL bootfiles.bin
        wdi-simple.exe            libwdi helper used to bind the WinUSB driver

    Two of those are easy to miss and fail confusingly at runtime:

    - the Cygwin DLLs. Copying rpiboot.exe alone gives a binary that dies on
      launch with no useful message.
    - bootfiles.bin. In the git repo it is a 25-byte symlink placeholder to
      ../firmware/bootfiles.bin; only the installer carries the real ~2.5 MB
      file. rpiboot fails with "No 'bootcode' files found" if it gets the
      placeholder, which is why this script size-checks it.

.PARAMETER Release
    Tag of the raspberrypi/usbboot release to pull rpiboot_setup.exe from.
    Pinned by default rather than tracking master, so a CI build is repeatable
    and an upstream change can't silently alter what we ship.

.PARAMETER Sha256
    Optional expected SHA-256 of rpiboot_setup.exe. The hash of what was
    downloaded is always printed, so this can be pinned after a first run.

.EXAMPLE
    .\scripts\fetch-rpiboot.ps1

.EXAMPLE
    .\scripts\fetch-rpiboot.ps1 -Release windows-v1.1 -Sha256 abc123...
#>

#Requires -Version 5.1
[CmdletBinding()]
param(
    [string] $Release = 'windows-v1.1',
    [string] $Sha256 = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$FlasherRoot = Split-Path -Parent $PSScriptRoot
$Dest = Join-Path $FlasherRoot 'src-tauri\binaries\rpiboot'
$Url = "https://github.com/raspberrypi/usbboot/releases/download/$Release/rpiboot_setup.exe"

$Work = Join-Path ([System.IO.Path]::GetTempPath()) ("rpiboot-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $Work | Out-Null

try {
    # --- 7-Zip -------------------------------------------------------------
    # Needed to unpack the NSIS installer. Present on GitHub windows runners;
    # locally, `winget install 7zip.7zip` or choco.
    $candidates = [System.Collections.Generic.List[string]]::new()
    $candidates.Add('7z')
    $candidates.Add('7za')
    foreach ($base in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if ($base) { $candidates.Add((Join-Path $base '7-Zip\7z.exe')) }
    }

    $sevenZip = $null
    foreach ($candidate in $candidates) {
        $cmd = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($cmd) { $sevenZip = $cmd.Source; break }
    }
    if (-not $sevenZip) {
        throw "7-Zip not found. Install it (winget install 7zip.7zip) and re-run."
    }
    Write-Host "==> Using 7-Zip: $sevenZip"

    # --- Download ----------------------------------------------------------
    $installer = Join-Path $Work 'rpiboot_setup.exe'
    Write-Host "==> Downloading $Url"
    # Invoke-WebRequest's progress bar makes a 34 MB download crawl in
    # Windows PowerShell 5.1; silencing it is a large speedup, not cosmetic.
    $previousProgress = $ProgressPreference
    $ProgressPreference = 'SilentlyContinue'
    try {
        Invoke-WebRequest -Uri $Url -OutFile $installer -UseBasicParsing
    }
    finally {
        $ProgressPreference = $previousProgress
    }

    $actual = (Get-FileHash -Path $installer -Algorithm SHA256).Hash.ToLower()
    Write-Host "==> SHA-256: $actual"
    if ($Sha256 -and $actual -ne $Sha256.ToLower()) {
        throw "Checksum mismatch for rpiboot_setup.exe.`n  expected: $($Sha256.ToLower())`n  actual:   $actual"
    }

    # --- Unpack ------------------------------------------------------------
    $extracted = Join-Path $Work 'extracted'
    Write-Host "==> Unpacking the installer"
    # 7-Zip warns (exit 1) about NSIS internals it doesn't model; that's not
    # fatal, so the real gate is whether the files we need turned up below.
    & $sevenZip x $installer "-o$extracted" -y | Out-Null
    if ($LASTEXITCODE -gt 1) {
        throw "7-Zip failed to unpack the installer (exit $LASTEXITCODE)."
    }

    # The NSIS layout isn't contractual, so locate everything by name rather
    # than by the path the current installer happens to use.
    function Find-One {
        param([string] $Name)
        $hit = Get-ChildItem -Path $extracted -Filter $Name -Recurse -File -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if (-not $hit) {
            throw "'$Name' not found in the installer. Its layout may have changed in release '$Release'."
        }
        return $hit.FullName
    }

    $gadgetDir = Get-ChildItem -Path $extracted -Filter 'mass-storage-gadget64' -Recurse -Directory -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $gadgetDir) {
        throw "'mass-storage-gadget64' not found in the installer (release '$Release')."
    }

    # --- Stage -------------------------------------------------------------
    Write-Host "==> Staging into $Dest"
    if (Test-Path $Dest) { Remove-Item -Path $Dest -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null

    foreach ($name in @('rpiboot.exe', 'cygusb-1.0.dll', 'cygwin1.dll', 'wdi-simple.exe')) {
        Copy-Item -Path (Find-One $name) -Destination (Join-Path $Dest $name) -Force
        Write-Host "    $name"
    }
    Copy-Item -Path $gadgetDir.FullName -Destination $Dest -Recurse -Force
    Write-Host "    mass-storage-gadget64\"

    # --- Verify ------------------------------------------------------------
    # bootfiles.bin is the one that silently ruins a build: the git placeholder
    # is 25 bytes, the real payload is megabytes. Catch it here rather than as
    # "No 'bootcode' files found" on a user's machine.
    $bootfiles = Join-Path $Dest 'mass-storage-gadget64\bootfiles.bin'
    if (-not (Test-Path $bootfiles)) {
        throw "mass-storage-gadget64\bootfiles.bin is missing from the staged files."
    }
    $size = (Get-Item $bootfiles).Length
    if ($size -lt 100KB) {
        throw "bootfiles.bin is only $size bytes - that's the git symlink placeholder, not the real boot payload."
    }

    Write-Host ""
    Write-Host "rpiboot staged in $Dest"
    Write-Host "  bootfiles.bin: $([math]::Round($size / 1MB, 2)) MB"
    Write-Host ""
    Write-Host "For dev, point the app at them:"
    Write-Host "  `$env:REACHY_RPIBOOT_BIN = '$Dest\rpiboot.exe'"
    Write-Host "  `$env:REACHY_RPIBOOT_DIR = '$Dest\mass-storage-gadget64'"
    Write-Host "  `$env:REACHY_WDI_SIMPLE_BIN = '$Dest\wdi-simple.exe'"
}
finally {
    Remove-Item -Path $Work -Recurse -Force -ErrorAction SilentlyContinue
}
