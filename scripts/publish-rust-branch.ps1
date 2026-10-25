# 把 Rust 重写版发布为 github.com/xiaoqvan/XQAnimeBot 的新分支（git 是危险操作，脚本只做加法，不 force-push）
# 用法：pwsh -File scripts/publish-rust-branch.ps1 [-Branch rust] [-Remote https://github.com/xiaoqvan/XQAnimeBot.git]
param(
    [string]$Branch = "rust",
    [string]$Remote = "https://github.com/xiaoqvan/XQAnimeBot.git"
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Test-Path ".git")) { throw "当前目录不是 git 仓库" }

# 1) 远端：没有 origin 才加，避免覆盖用户已有的远端配置
$existing = git remote get-url origin 2>$null
if (-not $existing) {
    git remote add origin $Remote
    Write-Host "已添加 origin -> $Remote"
} elseif ($existing -ne $Remote) {
    Write-Host "origin 已存在且不同（$existing），保留原值；如需切换请手动 git remote set-url"
}

# 2) 有未提交改动就提示，避免把半成品一起提交
$dirty = git status --porcelain
if ($dirty) {
    Write-Host "工作区有未提交改动："
    Write-Host $dirty
    $answer = Read-Host "仍然全部提交到分支 $Branch ? (y/N)"
    if ($answer -ne "y") { exit 1 }
}

# 3) 建立孤儿分支：保留 TS 历史在其它分支，Rust 版从这里重新开始
$current = git rev-parse --abbrev-ref HEAD
$hasBranch = git show-ref --verify --quiet "refs/heads/$Branch"
if ($hasBranch) {
    Write-Host "分支 $Branch 已存在，直接切过去"
    git checkout $Branch
} else {
    git checkout --orphan $Branch
    Write-Host "已从 $current 建孤儿分支 $Branch"
}

git add -A
git -c user.name="$(git config user.name)" commit -m "feat(rust): XQAnimeBot Rust 重写版（插件化 + RSS/匹配/存储层）"

# 4) 推送（首次带 -u）
git push -u origin $Branch
Write-Host "完成：https://github.com/xiaoqvan/XQAnimeBot/tree/$Branch"
