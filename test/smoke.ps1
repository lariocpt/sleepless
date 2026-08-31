# The Windows half of test/smoke.sh: ask the operating system whether it believes
# sleepless is holding a power request, and whether it stops believing that when the
# process is killed outright.
#
#     pwsh test/smoke.ps1 .\target\debug\sleepless.exe
param([string]$Bin = ".\target\debug\sleepless.exe")
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Bin)) { throw "FAIL: $Bin does not exist" }
$work = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
try {
    # A throwaway profile, so the suite cannot touch real settings.
    $env:USERPROFILE = $work
    $env:APPDATA = Join-Path $work "config"
    $env:LOCALAPPDATA = Join-Path $work "state"

    $out = & $Bin --always --smoke 2 | Out-String
    Write-Host $out
    if ($out -notmatch 'sleepless - ') { throw "FAIL: no status line" }
    # The status line is what a script greps; it has to stay ASCII.
    if ($out -match '[^\x00-\x7F]') { throw "FAIL: non-ASCII in the status line" }

    $p = Start-Process -PassThru -NoNewWindow $Bin '--always','--smoke','20'
    Start-Sleep -Seconds 3
    $req = powercfg /requests | Out-String
    Write-Host $req
    # Match the executable, so an unrelated request that merely mentions the word
    # cannot satisfy this the way "Anka sleeplessness" did on macOS.
    if ($req -notmatch 'sleepless\.exe') { throw "FAIL: no power request registered" }

    Stop-Process -Id $p.Id -Force
    Start-Sleep -Seconds 2
    if ((powercfg /requests | Out-String) -match 'sleepless\.exe') {
        throw "FAIL: the power request survived the process"
    }
    Write-Host "smoke: ok"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
