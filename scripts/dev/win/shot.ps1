# Save a screenshot of the primary screen. Run it on the user's desktop
# through runi.ps1 (an SSH session has no screen to capture).
param([string]$Out = "$env:USERPROFILE\shot.png")
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$i = New-Object System.Drawing.Bitmap $b.Width, $b.Height
[System.Drawing.Graphics]::FromImage($i).CopyFromScreen(0, 0, 0, 0, $i.Size)
$i.Save($Out)
