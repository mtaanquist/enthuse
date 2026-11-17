# Downloads a libmpv dev package for Windows and prepares it for the MSVC
# toolchain: headers, an import library named mpv.lib (what libmpv2-sys links
# against) and the runtime DLL.
#
# On CI it sets rustflags and PATH for later steps through GITHUB_ENV and
# GITHUB_PATH, so `cargo build` finds mpv.lib and `cargo test` finds the DLL.
#
# Local use: run it with -Dest, then set
#   CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-L native=<Dest>"
# and add <Dest> to PATH. Requires the GitHub CLI (gh) and 7-Zip.

param(
    [Parameter(Mandatory = $true)][string]$Dest,
    [string]$Tag = "",
    [string]$Repo = "shinchiro/mpv-winbuild-cmake"
)

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$Dest = (Resolve-Path $Dest).Path

if (-not (Test-Path (Join-Path $Dest "mpv.lib"))) {
    if (-not $Tag) {
        $Tag = gh release view --repo $Repo --json tagName --jq .tagName
        Write-Host "MPV_WINBUILD_TAG not set, using latest release: $Tag"
    }

    # x86_64 baseline build (not the -v3 variant, which needs AVX2).
    $asset = gh release view $Tag --repo $Repo --json assets --jq '.assets[].name' |
        Where-Object { $_ -match '^mpv-dev-x86_64-\d.*\.7z$' } |
        Select-Object -First 1
    if (-not $asset) { throw "No mpv-dev-x86_64 asset in release $Tag of $Repo" }
    Write-Host "Downloading $asset from $Repo@$Tag"

    $archive = Join-Path $env:RUNNER_TEMP $asset
    gh release download $Tag --repo $Repo --pattern $asset --output $archive --clobber
    7z x $archive "-o$Dest" -y | Out-Null

    $dll = Get-ChildItem -Path $Dest -Filter "libmpv-*.dll" -Recurse | Select-Object -First 1
    if (-not $dll) { throw "libmpv DLL not found in $asset" }
    if ($dll.DirectoryName -ne $Dest) { Copy-Item $dll.FullName $Dest }

    # Build mpv.lib from the DLL's export table using the MSVC tools.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $toolsVersion = (Get-Content (Join-Path $vsPath "VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt")).Trim()
    $bin = Join-Path $vsPath "VC\Tools\MSVC\$toolsVersion\bin\Hostx64\x64"

    $exports = & (Join-Path $bin "dumpbin.exe") /exports $dll.FullName
    $names = $exports |
        Where-Object { $_ -match '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]{8}\s+(mpv_\w+)' } |
        ForEach-Object { $Matches[1] }
    if ($names.Count -lt 10) { throw "Found only $($names.Count) mpv exports in $($dll.Name)" }

    $def = Join-Path $Dest "mpv.def"
    Set-Content -Path $def -Value (@("LIBRARY $($dll.Name)", "EXPORTS") + $names)
    & (Join-Path $bin "lib.exe") /nologo /def:$def /out:(Join-Path $Dest "mpv.lib") /machine:x64
    if ($LASTEXITCODE -ne 0) { throw "lib.exe failed" }
} else {
    Write-Host "Using cached libmpv in $Dest"
}

if ($env:GITHUB_ENV) {
    "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-L native=$Dest" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
    $Dest | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
}
Write-Host "libmpv ready in $Dest"
