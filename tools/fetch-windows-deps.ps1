# Descarga dependencias de Windows (URL fijas + sha256) a carpetas ignoradas.
# Uso: powershell -ExecutionPolicy Bypass -File tools\fetch-windows-deps.ps1
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$Bin = Join-Path $Root "src-tauri\binaries"
$Res = Join-Path $Root "src-tauri\resources"
New-Item -ItemType Directory -Force $Bin, $Res | Out-Null
$Tmp = Join-Path $env:TEMP ("musica-wdeps-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $Tmp | Out-Null

function Get-Verified($Url, $Out, $Sha) {
    Invoke-WebRequest -Uri $Url -OutFile $Out
    $got = (Get-FileHash -Algorithm SHA256 $Out).Hash.ToLower()
    if ($got -ne $Sha) { throw "sha256 de $Out no coincide: $got" }
}

$sevenZip = if (Get-Command 7z -ErrorAction SilentlyContinue) { "7z" } else { "C:\Program Files\7-Zip\7z.exe" }
$triple = "x86_64-pc-windows-msvc"

Get-Verified "https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.exe" `
    (Join-Path $Bin "yt-dlp-$triple.exe") `
    "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a"

Get-Verified "https://github.com/denoland/deno/releases/download/v2.9.8/deno-x86_64-pc-windows-msvc.zip" `
    (Join-Path $Tmp "deno.zip") `
    "cba84b26e0df5ae7f74fc66b86a95b866872a86292ebd84fa1e959927edd93bf"
Expand-Archive -Force (Join-Path $Tmp "deno.zip") (Join-Path $Tmp "deno")
Copy-Item -Force (Join-Path $Tmp "deno\deno.exe") (Join-Path $Bin "deno-$triple.exe")

Get-Verified "https://github.com/zhongfly/mpv-winbuild/releases/download/2026-10-14-7c4f662c08/ffmpeg-lgpl-x86_64-git-21441b340.7z" `
    (Join-Path $Tmp "ffmpeg.7z") `
    "13939d7166e1335792338a88666061326f552ed786e44f57e6bfaddd33a517c7"
& $sevenZip e -y "-o$Tmp\ff" (Join-Path $Tmp "ffmpeg.7z") ffmpeg.exe | Out-Null
Copy-Item -Force (Join-Path $Tmp "ff\ffmpeg.exe") (Join-Path $Bin "ffmpeg-$triple.exe")

Get-Verified "https://github.com/zhongfly/mpv-winbuild/releases/download/2026-10-14-7c4f662c08/mpv-dev-lgpl-x86_64-20261014-git-7c4f662c08.7z" `
    (Join-Path $Tmp "mpv.7z") `
    "37fd9c57c2c050e23162c08b6fe3eea0555a6b6e0b2d08470555834aea0a18d3"
& $sevenZip e -y "-o$Res" (Join-Path $Tmp "mpv.7z") libmpv-2.dll | Out-Null

Remove-Item -Recurse -Force $Tmp
Get-ChildItem $Bin, $Res | Select-Object FullName, Length
