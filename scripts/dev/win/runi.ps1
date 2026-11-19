# Start a program on the signed-in user's desktop from an SSH session.
# SSH runs in session 0 (no desktop), so this registers a one-off scheduled
# task with an interactive logon, starts it, and removes the task again. The
# program keeps running after the task is removed.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File runi.ps1 -Exe C:\path\app.exe -ArgLine "--flag"
param([string]$Exe, [string]$ArgLine = '', [string]$User = $env:USERNAME)
$act = if ($ArgLine) { New-ScheduledTaskAction -Execute $Exe -Argument $ArgLine } else { New-ScheduledTaskAction -Execute $Exe }
$s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
$p = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive
Register-ScheduledTask -TaskName kirun -Action $act -Settings $s -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName kirun
Start-Sleep 15
(Get-ScheduledTask -TaskName kirun).State
Unregister-ScheduledTask -TaskName kirun -Confirm:$false
