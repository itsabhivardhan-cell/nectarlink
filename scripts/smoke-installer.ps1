# SPDX-License-Identifier: GPL-3.0-or-later
<#
.SYNOPSIS
  Installs, starts, quits and uninstalls the packaged app silently, checking
  each step. For CI (needs admin; changes this PC), not for everyday use.
#>
param(
    [string]$Setup
)

$ErrorActionPreference = 'Stop'

if (-not $Setup) {
    $arch = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq [System.Runtime.InteropServices.Architecture]::Arm64) { 'arm64' } else { 'x64' }
    $pkgDir = Join-Path $PSScriptRoot '..\target\package'
    $match = Get-ChildItem (Join-Path $pkgDir "*-$arch-setup.exe") -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $match) {
        $match = Get-ChildItem (Join-Path $pkgDir '*-setup.exe') -ErrorAction SilentlyContinue | Select-Object -First 1
    }
    if ($match) { $Setup = $match.FullName }
}
if (-not $Setup -or -not (Test-Path $Setup)) { throw "no setup installer found to test under target\package" }
$dir = Join-Path $env:ProgramFiles 'Nectarlink'
$exe = Join-Path $dir 'nectarlink-desktop.exe'
$uninstallKey = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Nectarlink'
$shortcut = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\ZoopSync.lnk'
$data = if ($env:RUNNER_TEMP) { Join-Path $env:RUNNER_TEMP 'nectarlink-smoke' } else { Join-Path $env:TEMP 'nectarlink-smoke' }

function Check($what, [bool]$ok) {
    if (-not $ok) { throw "FAILED: $what" }
    Write-Host "ok: $what"
}
function FirewallRule { (netsh advfirewall firewall show rule name=Nectarlink) -match 'Nectarlink' }
# Whether an allow entry for `$sid` grants every bit of `$rights`.
function Test-TaskRights($sd, [string]$sid, [int]$rights) {
    [bool]($sd.DiscretionaryAcl | Where-Object {
        $_.AceQualifier -eq 'AccessAllowed' -and $_.SecurityIdentifier.Value -eq $sid -and
        ($_.AccessMask -band $rights) -eq $rights })
}
# Whether anyone but SYSTEM and administrators may write, delete or re-permission it.
function Test-OthersMayWrite($sd) {
    # FILE_WRITE_DATA, APPEND, WRITE_EA, WRITE_ATTRIBUTES, DELETE, WRITE_DAC,
    # WRITE_OWNER, GENERIC_ALL, GENERIC_WRITE.
    $write = 0x2 -bor 0x4 -bor 0x10 -bor 0x100 -bor 0x10000 -bor 0x40000 -bor 0x80000 -bor 0x10000000 -bor 0x40000000
    [bool]($sd.DiscretionaryAcl | Where-Object {
        $_.AceQualifier -eq 'AccessAllowed' -and
        $_.SecurityIdentifier.Value -notin @('S-1-5-18', 'S-1-5-32-544') -and
        ($_.AccessMask -band $write) -ne 0 })
}
function UpdateTask {
    $scheduler = New-Object -ComObject Schedule.Service
    $scheduler.Connect()
    try { $scheduler.GetFolder('\').GetTask('Nectarlink Update') } catch { $null }
}

Write-Host "Installing $Setup"
Start-Process $Setup -ArgumentList '/S' -Wait
Check "the app is installed" (Test-Path $exe)
Check "an uninstaller is installed" (Test-Path (Join-Path $dir 'uninstall.exe'))
Check "it's in Apps (uninstall entry)" (Test-Path $uninstallKey)
Check "it's in the Start menu" (Test-Path $shortcut)
Check "the firewall lets phones in" ([bool](FirewallRule))
Check "license notices are included" ((Get-Item (Join-Path $dir 'THIRD-PARTY-NOTICES.txt')).Length -gt 100KB)
$task = UpdateTask
Check "the update task is registered" ($null -ne $task)
Check "the update task runs the installed app as SYSTEM" (
    $task.Definition.Principal.UserId -match '^(SYSTEM|S-1-5-18|NT AUTHORITY\\SYSTEM)$' -and
    $task.Definition.Actions.Item(1).Path.Trim('"') -eq $exe -and
    $task.Definition.Actions.Item(1).Arguments -eq '--update-service')
# 4: the DACL. Signed-in users may read and run it, not change it. Windows
# reports the rights it maps them to (GRGX comes back as 0x1200a9), so check
# the access masks rather than the text.
$sddl = $task.GetSecurityDescriptor(4)
Write-Host "update task permissions: $sddl"
$sd = New-Object System.Security.AccessControl.RawSecurityDescriptor $sddl
Check "SYSTEM and administrators may change the update task" (
    ((Test-TaskRights $sd 'S-1-5-18' 0x1F01FF) -or (Test-TaskRights $sd 'S-1-5-18' 0x10000000)) -and
    ((Test-TaskRights $sd 'S-1-5-32-544' 0x1F01FF) -or (Test-TaskRights $sd 'S-1-5-32-544' 0x10000000)))
# FILE_EXECUTE, or GENERIC_EXECUTE if it isn't mapped.
Check "signed-in users may start the update task" (
    (Test-TaskRights $sd 'S-1-5-11' 0x20) -or (Test-TaskRights $sd 'S-1-5-11' 0x20000000))
Check "nobody else may change or delete the update task" (-not (Test-OthersMayWrite $sd))

# The installed copy starts on its own files and quits when asked.
$app = Start-Process $exe -ArgumentList '--minimized', '--data-dir', $data -PassThru
Start-Sleep 8
Check "the installed app keeps running" (-not $app.HasExited)
Start-Process $exe -ArgumentList '--quit', '--data-dir', $data -Wait
Check "the app quits when asked" ($app.WaitForExit(15000))
Check "it shut down cleanly" ($app.ExitCode -eq 0)

Write-Host "Uninstalling"
Start-Process (Join-Path $dir 'uninstall.exe') -ArgumentList '/S' -Wait
# The uninstaller runs from a copy in %TEMP%, so it returns before it's done.
$deadline = (Get-Date).AddSeconds(60)
while ((Test-Path $dir) -and (Get-Date) -lt $deadline) { Start-Sleep 1 }
Check "the app folder is gone" (-not (Test-Path $dir))
Check "the uninstall entry is gone" (-not (Test-Path $uninstallKey))
Check "the Start menu shortcut is gone" (-not (Test-Path $shortcut))
Check "the firewall rule is gone" (-not (FirewallRule))
Check "the update task is gone" ($null -eq (UpdateTask))
Write-Host "Installer smoke test passed."
# netsh reports "no rules match" with a failing exit code; that was the point.
exit 0
