# 拉取 Ultralight 2.0 SDK 并把运行时摆到 runtime/ultralight/
#
#   powershell -ExecutionPolicy Bypass -File scripts/fetch-ultralight.ps1
#   powershell -ExecutionPolicy Bypass -File scripts/fetch-ultralight.ps1 -Version 2.0.0-beta.2
#
# 只依赖系统自带的 tar（Windows 10+ 的 bsdtar 能解 7z）或已安装的 7-Zip。

param(
    [string]$Version = "latest",
    [string]$Platform = "windows-x64",
    [string]$OutDir = (Join-Path $PSScriptRoot "..\runtime\ultralight")
)

$ErrorActionPreference = "Stop"

$url = "https://ultralig.ht/api/v1/sdk/download?platform=$Platform&version=$Version"
$archive = Join-Path $env:TEMP "ultralight-sdk.7z"
$extract = Join-Path $env:TEMP "ultralight-sdk-$([guid]::NewGuid().ToString('N').Substring(0,8))"

Write-Host "下载 $url"
Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
Write-Host ("已下载 {0:N1} MB" -f ((Get-Item $archive).Length / 1MB))

New-Item -ItemType Directory -Path $extract -Force | Out-Null
$sevenZip = Get-Command 7z -ErrorAction SilentlyContinue
if ($sevenZip) {
    & $sevenZip.Source x -y "-o$extract" $archive | Out-Null
} else {
    # bsdtar（Windows 自带）支持读取 7z。
    tar -xf $archive -C $extract
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
Copy-Item (Join-Path $extract "bin\*.dll") $OutDir -Force
Copy-Item (Join-Path $extract "resources") $OutDir -Recurse -Force

Remove-Item -Recurse -Force $extract
Remove-Item -Force $archive

Write-Host "运行时已就位：$OutDir"
Get-ChildItem $OutDir | Select-Object Name, Length
