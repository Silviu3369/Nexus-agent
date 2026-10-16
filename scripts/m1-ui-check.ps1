<#
.SYNOPSIS
  End-to-end check of the M1 note flow through the real release window
  (notes live in Tools > Files - Notes of the command-center dashboard).

.DESCRIPTION
  Drives target\release\desktop.exe through Windows UI Automation (the
  interface screen readers use) in an isolated temporary profile, so it never
  touches %LOCALAPPDATA%\Nexus. Covers RUN-01 through the UI, UI-02 (inert
  content), LIFE-01 with a pending task, and a denial. Exits 1 on the first
  failed check. Build first:
    cargo build -p nexus-host --release --locked
    pnpm --filter @nexus/desktop tauri build --no-bundle
#>
param(
  [string]$Root = (Resolve-Path "$PSScriptRoot\..").Path,
  # Folder holding desktop.exe with nexus-host.exe next to it.
  [string]$Bin = (Join-Path $Root "target\release")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$S = [System.Windows.Automation.TreeScope]
$True_ = [System.Windows.Automation.Condition]::TrueCondition

$Bin = (Resolve-Path $Bin).Path
$exe = Join-Path $Bin "desktop.exe"
$hostExe = Join-Path $Bin "nexus-host.exe"
foreach ($f in $exe, $hostExe) { if (-not (Test-Path $f)) { throw "missing $f - build the release first" } }

$profileDir = Join-Path $env:TEMP ("nexus-ui-check-" + [guid]::NewGuid())
New-Item -ItemType Directory $profileDir | Out-Null
# Inherited by desktop.exe and by the host it launches: both use this profile
# and its own pipe name, never the user's real one.
$env:NEXUS_PROFILE_DIR = $profileDir

$hostile = '<script>alert(1)</script> <img src="https://example.com/x.png"> [x](javascript:alert(1))'
$failures = 0

function Check([bool]$ok, [string]$what) {
  if ($ok) { Write-Output "PASS  $what" } else { Write-Output "FAIL  $what"; $script:failures++ }
}

function Get-Window([int]$procId) {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::ProcessIdProperty, $procId)
  for ($i = 0; $i -lt 50; $i++) {
    $w = $A::RootElement.FindFirst($S::Children, $cond)
    if ($w) { return $w }
    Start-Sleep -Milliseconds 200
  }
  throw "window for pid $procId did not appear"
}

function Get-Names($win) {
  $win.FindAll($S::Descendants, $True_) | ForEach-Object { $_.Current.Name } | Where-Object { $_ }
}

function Wait-Text($win, [string]$text, [int]$seconds = 15) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    if ((Get-Names $win) -contains $text) { return $true }
    Start-Sleep -Milliseconds 250
  }
  return $false
}

function Wait-Contains($win, [string]$fragment, [int]$seconds = 15) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $hit = Get-Names $win | Where-Object { $_.Contains($fragment) } | Select-Object -First 1
    if ($hit) { return $hit }
    Start-Sleep -Milliseconds 250
  }
  return $null
}

function Find-Named($win, [string]$name, $type) {
  $byName = New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, $name)
  $byType = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, $type)
  $win.FindFirst($S::Descendants, (New-Object System.Windows.Automation.AndCondition($byName, $byType)))
}

function Invoke-Element($element) {
  $pattern = $null
  if ($element.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
    $pattern.Invoke()
  } elseif ($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
    $pattern.Toggle()
  } else {
    throw "element '$($element.Current.Name)' cannot be activated"
  }
}

function Click-Button($win, [string]$name) {
  $deadline = (Get-Date).AddSeconds(10)
  while ((Get-Date) -lt $deadline) {
    $b = Find-Named $win $name ([System.Windows.Automation.ControlType]::Button)
    if ($b -and $b.Current.IsEnabled) {
      Invoke-Element $b
      return
    }
    Start-Sleep -Milliseconds 200
  }
  throw "button '$name' not found or never enabled"
}

function Set-Field($win, [string]$label, [string]$value) {
  $e = Find-Named $win $label ([System.Windows.Automation.ControlType]::Edit)
  if (-not $e) { throw "field '$label' not found" }
  $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($value)
}

function Note-Files { @(Get-ChildItem -Path (Join-Path $profileDir "artifacts") -Recurse -Filter *.md -ErrorAction SilentlyContinue | Where-Object { $_.DirectoryName -notlike "*\.staging" }) }

function Wait-Files([int]$count, [int]$seconds = 15) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) { if ((Note-Files).Count -eq $count) { return $true }; Start-Sleep -Milliseconds 250 }
  return $false
}

# A button whose accessible name contains $fragment (rows carry their status).
function Click-Like($win, [string]$fragment, [int]$seconds = 10) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $b = $win.FindAll($S::Descendants, $True_) | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -like "*$fragment*" -and $_.Current.IsEnabled } | Select-Object -First 1
    if ($b) { Invoke-Element $b; return }
    Start-Sleep -Milliseconds 200
  }
  throw "no button containing '$fragment'"
}

function Open-Notes($win) {
  Click-Button $win "Tools"
  Click-Like $win "Files · Notes"
}

function New-Note($win, [string]$title, [string]$body) {
  Set-Field $win "Title" $title
  Set-Field $win "Content" $body
  Click-Button $win "Prepare note"
}

$ui = $null
$started = Get-Date
try {
  # --- start the host from the UI ---
  $ui = Start-Process $exe -PassThru
  $win = Get-Window $ui.Id
  Check (Wait-Text $win "Start Host") "fresh profile shows the host as stopped"
  Click-Button $win "Start Host"
  Check (Wait-Text $win "Nexus.Host") "the UI launched the host and connected"
  Open-Notes $win
  Check (Wait-Text $win "No notes yet.") "empty activity state is shown"

  # --- RUN-01 + UI-02 through the window ---
  New-Note $win "Test note" $hostile
  Check (Wait-Text $win "What will be written") "an exact preview is shown before anything is written"
  $expected = "# Test note`n`n$hostile`n"
  Check (@(Get-Names $win | Where-Object { $_.Contains($hostile) }).Count -gt 0) "hostile markup appears as literal text in the preview"
  Check ((Note-Files).Count -eq 0) "no file exists before approval"
  Click-Button $win "Approve and create"
  Check (Wait-Files 1) "approving creates exactly one note"
  $file = (Note-Files)[0].FullName
  Check ([System.IO.File]::ReadAllText($file, [System.Text.UTF8Encoding]::new($false)) -ceq $expected) "the file holds exactly the approved bytes"
  Check ($file.StartsWith($profileDir)) "the note is inside the isolated profile"
  Click-Button $win "Show content"
  Check (Wait-Text $win "Verified content of the note") "verified content can be shown"

  # --- LIFE-01: close the UI while a task waits, reopen, finish it ---
  New-Note $win "Second note" "content 2"
  Check (Wait-Text $win "Awaiting your approval") "second note waits for approval"
  $hostProc = Get-Process nexus-host | Where-Object { $_.Path -eq $hostExe -and $_.StartTime -ge $started } | Select-Object -First 1
  $ui.CloseMainWindow() | Out-Null
  $ui.WaitForExit(10000) | Out-Null
  Start-Sleep -Seconds 2
  Check ($ui.HasExited) "the window closed"
  Check (-not (Get-Process -Id $hostProc.Id -ErrorAction SilentlyContinue).HasExited) "the host survived closing the UI with a pending task"

  $ui = Start-Process $exe -PassThru
  $win = Get-Window $ui.Id
  Check (Wait-Text $win "Nexus.Host") "the reopened UI reconnected"
  Open-Notes $win
  $row = $null
  $deadline = (Get-Date).AddSeconds(10)
  while (-not $row -and (Get-Date) -lt $deadline) {
    $row = $win.FindAll($S::Descendants, $True_) | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -like "*Second note*" } | Select-Object -First 1
    if (-not $row) { Start-Sleep -Milliseconds 250 }
  }
  Check ($null -ne $row) "the pending task is still listed after reopening"
  Check ($row.Current.Name -like "*Awaiting your approval*") "its state was recovered, not reset"
  Invoke-Element $row
  Check (Wait-Text $win "What will be written") "its exact preview is shown again"
  Click-Button $win "Approve and create"
  Check (Wait-Files 2) "approving after reopening creates exactly the second note"
  Check ((Get-Process nexus-host | Where-Object { $_.Path -eq $hostExe -and $_.StartTime -ge $started }).Count -eq 1) "still a single host instance"

  # --- denial writes nothing ---
  New-Note $win "Third note" "I don't want it"
  Check (Wait-Text $win "What will be written") "third note shows its preview"
  Click-Button $win "Deny"
  Check (Wait-Text $win "Denied") "denial is shown"
  Start-Sleep -Seconds 1
  Check ((Note-Files).Count -eq 2) "denial wrote no file"

  # --- BACKUP-01 through the UI and the real restore command ---
  Click-Button $win "Settings"
  Click-Button $win "Back up now"
  Check ($null -ne (Wait-Contains $win "Backup created: 2 notes")) "the UI reports a backup of both notes"
  $backupPath = Wait-Contains $win (Join-Path $profileDir "backups")
  Check ($null -ne $backupPath -and (Test-Path (Join-Path $backupPath "manifest.json"))) "the backup and its manifest exist inside the profile"
  $manifest = Get-Content -Raw -Encoding UTF8 (Join-Path $backupPath "manifest.json") | ConvertFrom-Json
  Check (@($manifest.artifacts).Count -eq 2) "the manifest lists both notes with their hashes"

  $restoredDir = Join-Path $env:TEMP ("nexus-ui-restored-" + [guid]::NewGuid())
  $script:restoredDir = $restoredDir
  & $hostExe restore $backupPath $restoredDir | Out-Null
  Check ($LASTEXITCODE -eq 0) "nexus-host restore into a new profile succeeded"
  Check ((Test-Path (Join-Path $restoredDir "restored.json")) -and @(Get-ChildItem (Join-Path $restoredDir "artifacts") -Recurse -Filter *.md).Count -eq 2) "the restored profile holds both notes and is marked restored"

  $ui.CloseMainWindow() | Out-Null; $ui.WaitForExit(10000) | Out-Null
  Get-Process nexus-host -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $hostExe -and $_.StartTime -ge $started } | Stop-Process -Force
  Start-Sleep -Milliseconds 500

  $env:NEXUS_PROFILE_DIR = $restoredDir
  $restoreStart = Get-Date
  $ui = Start-Process $exe -PassThru
  $win = Get-Window $ui.Id
  Check (Wait-Text $win "Start Host") "the restored profile has no host running yet"
  Click-Button $win "Start Host"
  Check ($null -ne (Wait-Contains $win "Restored from a backup")) "the UI states that the restored profile executes nothing"
  Open-Notes $win
  Check ($null -ne (Wait-Contains $win "Third note")) "the restored history is visible"
  Check ($null -ne (Wait-Contains $win "Denied")) "restored runs keep their states"
}
finally {
  if ($ui -and -not $ui.HasExited) { $ui.CloseMainWindow() | Out-Null; $ui.WaitForExit(10000) | Out-Null }
  Get-Process nexus-host -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $hostExe -and $_.StartTime -ge $started } | Stop-Process -Force
  Start-Sleep -Milliseconds 500
  Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue
  if ($script:restoredDir) { Remove-Item -Recurse -Force $script:restoredDir -ErrorAction SilentlyContinue }
  Remove-Item Env:NEXUS_PROFILE_DIR -ErrorAction SilentlyContinue
}

if ($failures -gt 0) { Write-Output "$failures check(s) failed"; exit 1 }
Write-Output "all checks passed"
