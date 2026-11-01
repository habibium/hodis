# Installs hodis from its GitHub releases: the latest, or the one
# HODIS_VERSION names (such as v0.1.0), into HODIS_INSTALL_DIR, which is
# %LOCALAPPDATA%\Programs\hodis unless set, and puts that on the user's PATH.
#
#   irm https://github.com/habibium/hodis/releases/latest/download/hodis-installer.ps1 | iex

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 may not offer TLS 1.2 unasked, and GitHub requires it.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$repo = 'habibium/hodis'
$version = if ($env:HODIS_VERSION) { $env:HODIS_VERSION } else { 'latest' }
$dir = if ($env:HODIS_INSTALL_DIR) { $env:HODIS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\hodis' }

# Windows on Arm runs the x64 build through its emulation.
$target = 'x86_64-pc-windows-msvc'
$archive = "hodis-$target.zip"
$url = if ($version -eq 'latest') {
    "https://github.com/$repo/releases/latest/download"
} else {
    "https://github.com/$repo/releases/download/$version"
}

$temp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    Write-Host "Downloading hodis for $target"
    Invoke-WebRequest -UseBasicParsing -Uri "$url/$archive" -OutFile (Join-Path $temp $archive)
    Invoke-WebRequest -UseBasicParsing -Uri "$url/$archive.sha256" -OutFile (Join-Path $temp "$archive.sha256")
    $expected = (Get-Content -Raw (Join-Path $temp "$archive.sha256")).Trim().Split()[0]
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $temp $archive)).Hash
    if ($actual -ne $expected) { throw 'the download does not match its checksum' }

    Expand-Archive -Path (Join-Path $temp $archive) -DestinationPath $temp
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -Force (Join-Path $temp "hodis-$target\hodis.exe") (Join-Path $dir 'hodis.exe')
} finally {
    Remove-Item -Recurse -Force $temp
}

$installed = & (Join-Path $dir 'hodis.exe') --version
Write-Host "Installed $installed into $dir"

$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($path -split ';') -contains $dir)) {
    $entries = @($path, $dir) | Where-Object { $_ }
    [Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
    Write-Host "Added $dir to your PATH; open a new terminal to run hodis by name."
}
