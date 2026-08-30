# Photograph the window, without a person and without the compositor.
#
# **Reading the screen from outside does not work on this window.** eframe draws
# with OpenGL, and an unoccluded GL surface goes to the display without passing
# through the desktop compositor -- so `CopyFromScreen` comes back black,
# `PrintWindow` comes back white, and the only capture that ever succeeded did so
# because a browser happened to be sitting on top of the window at the time.
# Three passes over this design were made without anyone seeing it, and a glyph
# that rendered as `?` in every category heading survived all three.
#
# So the app takes the picture: `TGX_SHOT` makes it ask egui for the frame it
# just drew, write it as raw RGBA, and quit. See `shell::shot`. This turns that
# into a PNG.
#
#   tools\shot.ps1 -View chats -Out shots\chats.png
#
# The app connects to Telegram on startup exactly as it does when launched by
# hand -- it signs in with the stored session if there is one, so the chat list
# in the picture is a real account's. It does not export anything.

[CmdletBinding()]
param(
    [ValidateSet('chats', 'settings', 'run')]
    [string]$View = 'chats',
    [string]$Out = "$env:TEMP\tgx-shot.png",
    [string]$Exe = "dist\TelegramExporter.exe",
    # Long enough for a sign-in and a chat list; the app quits by itself as soon
    # as it has written the frame, so this is only a ceiling.
    [int]$TimeoutSeconds = 40
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$raw = [System.IO.Path]::Combine($env:TEMP, "tgx-shot-$View.raw")
Remove-Item $raw -Force -ErrorAction SilentlyContinue

$env:TGX_SHOT = $raw
$env:TGX_SHOT_VIEW = $View
try {
    $p = Start-Process -FilePath $Exe -PassThru
    if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        throw "the window did not write a frame within ${TimeoutSeconds}s"
    }
}
finally {
    Remove-Item Env:TGX_SHOT, Env:TGX_SHOT_VIEW -ErrorAction SilentlyContinue
}

if (-not (Test-Path $raw)) { throw "no frame at $raw -- see TelegramExporterData\tgx.log" }

$bytes = [System.IO.File]::ReadAllBytes($raw)
if ($bytes.Length -lt 12 -or [System.Text.Encoding]::ASCII.GetString($bytes, 0, 4) -ne 'TGXS') {
    throw "$raw is not a frame this script wrote"
}
$w = [BitConverter]::ToUInt32($bytes, 4)
$h = [BitConverter]::ToUInt32($bytes, 8)
$expected = 12 + $w * $h * 4
if ($bytes.Length -ne $expected) { throw "expected $expected bytes for ${w}x${h}, got $($bytes.Length)" }

# RGBA on the wire, BGRA in a GDI+ bitmap, so the two colour channels swap.
$pixels = New-Object byte[] ($w * $h * 4)
for ($i = 0; $i -lt $w * $h; $i++) {
    $s = 12 + $i * 4
    $d = $i * 4
    $pixels[$d] = $bytes[$s + 2]
    $pixels[$d + 1] = $bytes[$s + 1]
    $pixels[$d + 2] = $bytes[$s]
    $pixels[$d + 3] = $bytes[$s + 3]
}

$bmp = New-Object System.Drawing.Bitmap ([int]$w), ([int]$h), ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$rect = New-Object System.Drawing.Rectangle 0, 0, ([int]$w), ([int]$h)
$data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::WriteOnly, $bmp.PixelFormat)
[System.Runtime.InteropServices.Marshal]::Copy($pixels, 0, $data.Scan0, $pixels.Length)
$bmp.UnlockBits($data)

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Remove-Item $raw -Force -ErrorAction SilentlyContinue

"$Out  (${w}x${h}, $View)"
