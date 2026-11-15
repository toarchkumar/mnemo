# Install mnemo on Windows, the encrypted single-file memory for AI agents.
#
#   irm https://toarchkumar.github.io/mnemo/install.ps1 | iex
#
# The same steps as install.sh (docs/design/12.2-onboard.md, section 1):
# the latest release (or $env:MNEMO_VERSION = "0.16.0"), its binary and
# SHA256SUMS over HTTPS into a new temporary directory, the SHA-256 checked
# (a mismatch stops it, and nothing is installed), the build's attestation
# checked when the GitHub CLI (`gh`) is installed and signed in, then
# %LOCALAPPDATA%\Programs\mnemo\mnemo.exe, replaced in a single move.
#
# The SHA-256 catches a corrupt or cut-short download; SHA256SUMS comes from
# the same release, so only the attestation catches swapped files.
#
# $env:MNEMO_INSTALL_BASE (for the release workflow's own check) names a
# local directory holding the binary and SHA256SUMS; then nothing is
# downloaded, and the attestation is skipped.
#
# Everything is in one function, called on the last line, so a download cut
# short runs nothing.

function Install-Mnemo {
    $ErrorActionPreference = 'Stop'
    $repo = 'toarchkumar/mnemo'

    # `throw`, not `exit`: under `irm | iex`, `exit` would close the
    # user's PowerShell.
    function Fail($msg) {
        throw "mnemo install: $msg"
    }

    if ([System.Environment]::Is64BitOperatingSystem -eq $false -or
        $env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
        Fail "there's a mnemo build for 64-bit x86 Windows only"
    }
    $target = 'x86_64-pc-windows-msvc'

    $base = $env:MNEMO_INSTALL_BASE
    if ($base) {
        if (-not [System.IO.Path]::IsPathRooted($base) -or $base -match '^[a-zA-Z][a-zA-Z0-9+.-]*://') {
            Fail "MNEMO_INSTALL_BASE must be an absolute path to a local directory: $base"
        }
        if (-not (Test-Path -LiteralPath $base -PathType Container)) {
            Fail "MNEMO_INSTALL_BASE isn't a directory: $base"
        }
    }

    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.ServicePointManager]::SecurityProtocol

    $version = $env:MNEMO_VERSION
    if (-not $version) {
        if ($base) { Fail "with MNEMO_INSTALL_BASE, set MNEMO_VERSION too" }
        # The latest release's version, from the redirect of /releases/latest.
        $req = [System.Net.HttpWebRequest]::Create("https://github.com/$repo/releases/latest")
        $req.AllowAutoRedirect = $false
        $resp = $req.GetResponse()
        $location = $resp.Headers['Location']
        $resp.Close()
        if (-not $location) { Fail "couldn't find the latest release" }
        $version = ($location -split '/')[-1] -replace '^v', ''
    }
    if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
        Fail "not a version: '$version' (want something like 0.16.0)"
    }

    $name = "mnemo-v$version-$target.exe"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("mnemo-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $file = Join-Path $tmp $name
        $sums = Join-Path $tmp 'SHA256SUMS'
        if ($base) {
            if (-not (Test-Path -LiteralPath (Join-Path $base $name))) { Fail "no $name in $base" }
            if (-not (Test-Path -LiteralPath (Join-Path $base 'SHA256SUMS'))) { Fail "no SHA256SUMS in $base" }
            Copy-Item -LiteralPath (Join-Path $base $name) $file
            Copy-Item -LiteralPath (Join-Path $base 'SHA256SUMS') $sums
        } else {
            Write-Output "downloading mnemo $version for $target"
            $url = "https://github.com/$repo/releases/download/v$version"
            Invoke-WebRequest -UseBasicParsing -Uri "$url/$name" -OutFile $file
            Invoke-WebRequest -UseBasicParsing -Uri "$url/SHA256SUMS" -OutFile $sums
        }

        $want = $null
        foreach ($line in Get-Content -LiteralPath $sums) {
            $parts = $line -split '\s+', 2
            if ($parts.Count -eq 2 -and ($parts[1] -eq $name -or $parts[1] -eq "*$name")) {
                $want = $parts[0].ToLower()
            }
        }
        if (-not $want) { Fail "SHA256SUMS doesn't list $name; nothing was installed" }
        $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLower()
        if ($got -ne $want) {
            Fail "the SHA-256 doesn't match SHA256SUMS ($got, not $want); nothing was installed"
        }

        $ghReady = $false
        if (-not $base -and (Get-Command gh -ErrorAction SilentlyContinue)) {
            & gh auth status *> $null
            $ghReady = ($LASTEXITCODE -eq 0)
        }
        if ($base) {
            Write-Output "checked the SHA-256 (a local MNEMO_INSTALL_BASE: the attestation was skipped)"
        } elseif ($ghReady) {
            & gh attestation verify $file --repo $repo | Out-Null
            if ($LASTEXITCODE -ne 0) { Fail "the build's attestation didn't verify; nothing was installed" }
            Write-Output "checked the SHA-256 and the build's attestation"
        } else {
            Write-Output "checked the SHA-256 only (install and sign in to ``gh`` to also check the build's attestation)"
        }

        $dir = Join-Path $env:LOCALAPPDATA 'Programs\mnemo'
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $dest = Join-Path $dir 'mnemo.exe'
        # Copy beside the target, then one move over the old one.
        $staged = Join-Path $dir ".mnemo.new.$PID.exe"
        Copy-Item -LiteralPath $file $staged
        Move-Item -Force -LiteralPath $staged $dest

        & $dest --version
        if ($LASTEXITCODE -ne 0) { Fail "the installed mnemo didn't run" }
        Write-Output "installed $dest"
        $onPath = ($env:PATH -split ';') -contains $dir
        if ($onPath) {
            Write-Output "Next: mnemo onboard"
        } else {
            Write-Output "$dir isn't on your PATH. Add it for your user with:"
            Write-Output "  [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';$dir', 'User')"
            Write-Output "Next: & '$dest' onboard"
        }
    } finally {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
}

Install-Mnemo
