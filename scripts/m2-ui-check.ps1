<#
.SYNOPSIS
  End-to-end check of the M2 chat through the real release window, against
  the scripted Gemini test fake (no request reaches Google).

.DESCRIPTION
  Starts the test fake (crates/nexus-host/examples/fake_gemini_server.rs), a
  DEBUG nexus-host pointed at it with NEXUS_GEMINI_BASE_URL (release hosts
  ignore that variable) and the release desktop.exe, all on an isolated
  temporary profile; drives the window through Windows UI Automation.
  Covers: refused key not stored, key stored only in the Windows vault,
  model choice, persona version, streamed reply, a proposed note that waits
  on the approval card, stopping a reply, a quota error, reopening the
  window, a task given from the command bar with its steps, the Nexus Core
  path and the activity log, a temporary outage tried again, projects, a
  fact the agent proposes that is sent only after you approve it, a fact
  you add, search and delete, and removing the connection with its vault
  entry. Build first:
    cargo build -p nexus-host --locked
    cargo build -p nexus-host --example fake_gemini_server --locked
    pnpm --filter @nexus/desktop tauri build --no-bundle
#>
param(
  [string]$Root = (Resolve-Path "$PSScriptRoot\..").Path,
  # Folder holding the release desktop.exe (another target dir works too).
  [string]$Bin = (Join-Path $Root "target\release")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$S = [System.Windows.Automation.TreeScope]
$True_ = [System.Windows.Automation.Condition]::TrueCondition

$exe = Join-Path (Resolve-Path $Bin).Path "desktop.exe"
$hostExe = Join-Path $Root "target\debug\nexus-host.exe"
$fakeExe = Join-Path $Root "target\debug\examples\fake_gemini_server.exe"
foreach ($f in $exe, $hostExe, $fakeExe) { if (-not (Test-Path $f)) { throw "missing $f - build first (see the header)" } }

$profileDir = Join-Path $env:TEMP ("nexus-m2-ui-check-" + [guid]::NewGuid())
New-Item -ItemType Directory $profileDir | Out-Null
$env:NEXUS_PROFILE_DIR = $profileDir

# Test values only; the fake accepts exactly $testKey.
# Shaped like Google keys (AIza...), so the provider is detected from the format.
$testKey = "AIzaSENTINEL-ui-check-" + [guid]::NewGuid()
$wrongKey = "AIzaWRONG-ui-check-" + [guid]::NewGuid()
$failures = 0

function Check([bool]$ok, [string]$what) {
  if ($ok) { Write-Output "PASS  $what" } else { Write-Output "FAIL  $what"; $script:failures++ }
}

# Same FNV-1a 64 as SecretVault::for_profile: the profile's vault prefix.
function Vault-Prefix([string]$path) {
  $BI = [System.Numerics.BigInteger]
  $mod = $BI::Pow(2, 64)
  $prime = $BI::Parse("1099511628211")
  $h = $BI::Parse("14695981039346656037")
  foreach ($b in [System.Text.Encoding]::UTF8.GetBytes($path.ToLowerInvariant())) {
    $h = $BI::Remainder($BI::Multiply($BI::op_ExclusiveOr($h, [System.Numerics.BigInteger]$b), $prime), $mod)
  }
  $hex = $h.ToString("x")
  if ($hex.Length -gt 16) { $hex = $hex.Substring($hex.Length - 16) }
  "Nexus/" + $hex.PadLeft(16, '0') + "/"
}
$prefix = Vault-Prefix $profileDir
function Vault-Entries { @(cmdkey /list | Select-String -SimpleMatch "target=$prefix" | ForEach-Object { ($_.Line -split "target=")[1].Trim() }) }

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
  } elseif ($element.TryGetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$pattern)) {
    # Disclosure buttons (aria-expanded) are exposed as expand/collapse.
    if ($pattern.Current.ExpandCollapseState -eq [System.Windows.Automation.ExpandCollapseState]::Collapsed) { $pattern.Expand() } else { $pattern.Collapse() }
  } else {
    throw "element '$($element.Current.Name)' cannot be activated"
  }
}

function Click-Button($win, [string]$name, [int]$seconds = 10) {
  $deadline = (Get-Date).AddSeconds($seconds)
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
  $deadline = (Get-Date).AddSeconds(10)
  while ((Get-Date) -lt $deadline) {
    $e = Find-Named $win $label ([System.Windows.Automation.ControlType]::Edit)
    if ($e) {
      $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($value)
      return
    }
    Start-Sleep -Milliseconds 200
  }
  throw "field '$label' not found"
}

# A button whose accessible name contains $fragment.
function Find-Like($win, [string]$fragment) {
  $win.FindAll($S::Descendants, $True_) | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -like "*$fragment*" } | Select-Object -First 1
}

function Wait-Like($win, [string]$fragment, [int]$seconds = 15) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $b = Find-Like $win $fragment
    if ($b) { return $b }
    Start-Sleep -Milliseconds 250
  }
  return $null
}

function Wait-Match($win, [string]$regex, [int]$seconds = 15) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $hit = Get-Names $win | Where-Object { $_ -match $regex } | Select-Object -First 1
    if ($hit) { return $hit }
    Start-Sleep -Milliseconds 250
  }
  return $null
}

function Send-Chat($win, [string]$text) {
  Set-Field $win "Message" $text
  Click-Button $win "Send" 20
}

function Note-Files { @(Get-ChildItem -Path (Join-Path $profileDir "artifacts") -Recurse -Filter *.md -ErrorAction SilentlyContinue | Where-Object { $_.DirectoryName -notlike "*\.staging" }) }

function Wait-Files([int]$count, [int]$seconds = 15) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) { if ((Note-Files).Count -eq $count) { return $true }; Start-Sleep -Milliseconds 250 }
  return $false
}

$ui = $null
$hostProc = $null
$fake = $null
$fakeOut = Join-Path $profileDir "fake-gemini.url"
try {
  # --- the test fake, then a debug host pointed at it ---
  $fake = Start-Process $fakeExe -ArgumentList $testKey -NoNewWindow -PassThru -RedirectStandardOutput $fakeOut
  $baseUrl = $null
  for ($i = 0; $i -lt 50 -and -not $baseUrl; $i++) {
    Start-Sleep -Milliseconds 100
    $baseUrl = (Get-Content $fakeOut -ErrorAction SilentlyContinue | Select-Object -First 1)
  }
  if (-not $baseUrl -or -not $baseUrl.StartsWith("http://127.0.0.1:")) { throw "the test fake did not start" }
  $env:NEXUS_GEMINI_BASE_URL = $baseUrl
  $hostProc = Start-Process $hostExe -WindowStyle Hidden -PassThru
  Remove-Item Env:NEXUS_GEMINI_BASE_URL

  $ui = Start-Process $exe -PassThru
  $win = Get-Window $ui.Id
  Check (Wait-Text $win "Nexus.Host") "the window connected to the host of the test profile"

  # --- AI-03: nothing can be sent without a chosen connection and model ---
  Check ($null -ne (Wait-Contains $win "Nexus sends nothing until you choose a connection and a model")) "the command bar says no model is chosen yet"
  $run = Find-Named $win "Run" ([System.Windows.Automation.ControlType]::Button)
  Check ($run -and -not $run.Current.IsEnabled) "the command bar cannot run a task without a connection"
  Click-Button $win "Tasks"
  $send = $null
  $deadline = (Get-Date).AddSeconds(10)
  while (-not $send -and (Get-Date) -lt $deadline) { $send = Find-Named $win "Send" ([System.Windows.Automation.ControlType]::Button); if (-not $send) { Start-Sleep -Milliseconds 200 } }
  Check ($send -and -not $send.Current.IsEnabled) "the task composer is disabled without a connection"

  # --- a refused key is not stored ---
  Click-Button $win "Set up a model"
  Check (Wait-Text $win "AI connections") "Models shows the AI connections"
  Check ($null -ne (Wait-Contains $win "No adapter")) "a provider without an adapter is shown as such"
  Set-Field $win "API key" $wrongKey
  Check ($null -ne (Wait-Contains $win "Detected: Google Gemini")) "the provider is read from the key's format"
  Check ($null -ne (Wait-Contains $win "personal data, passwords")) "the free-tier notice is shown for a Google key"
  Click-Button $win "Verify and save"
  Check ($null -ne (Wait-Contains $win "Authentication was refused" 20)) "a key the provider refuses is reported"
  Check ((Wait-Text $win "No connections yet.")) "the refused key created no connection"
  Check ((Vault-Entries).Count -eq 0) "the refused key is not in Credential Manager"
  Check (-not (Get-Names $win | Where-Object { $_.Contains($wrongKey) })) "the refused key is not shown anywhere"

  # --- a valid key goes to the vault only ---
  Set-Field $win "API key" $testKey
  Click-Button $win "Verify and save"
  Check ($null -ne (Wait-Contains $win "Active" 20)) "the checked key created the active connection"
  Check (Wait-Text $win "in the Windows vault") "the UI reports the key as held by the Windows vault"
  Check ((Vault-Entries).Count -eq 1) "exactly one Credential Manager entry exists for this profile"
  Check (-not (Get-Names $win | Where-Object { $_.Contains($testKey) })) "the key is not shown anywhere in the window"

  # --- model choice (no fallback: nothing runs until a model is chosen) ---
  Click-Button $win "Load models" 20
  Click-Button $win "Use this model" 20
  Check (Wait-Text $win "gemini-fake-flash") "the chosen model is shown on the connection"
  Check ($null -ne (Wait-Contains $win "Chosen by you")) "routing says why this model is used"

  # --- persona version ---
  Click-Button $win "Settings"
  Set-Field $win "Assistant name" "Ada"
  Click-Button $win "Save persona"
  Check ($null -ne (Wait-Contains $win "Saved as version 2")) "saving the persona creates version 2"

  # --- AI-01: streamed reply from the chosen model, in a new task ---
  Click-Button $win "Tasks"
  Send-Chat $win "Hello!"
  Check (Wait-Text $win "Hello! I'm your Nexus assistant.") "the streamed reply is shown"
  Check ($null -ne (Wait-Contains $win "gemini-fake-flash · persona v2")) "the run records its model and persona version"
  Check ($null -ne (Wait-Like $win "Model call")) "the model call is shown as a step of the run"

  # --- AI-02: a proposed note waits on the approval card ---
  Send-Chat $win "Save the plan as a note"
  Check ($null -ne (Wait-Contains $win "Approval required")) "the proposal raises an approval card"
  Check ($null -ne (Wait-Contains $win 'Create note “Chat plan”')) "the card names the exact action"
  Check ($null -ne (Wait-Contains $win "Ada · task")) "the card says which agent and task asked"
  Check ($null -ne (Wait-Like $win 'Propose note "Chat plan"')) "the proposal is shown as a step"
  Check ($null -ne (Wait-Contains $win "1 new file")) "the card states what it affects"
  Click-Button $win "Inspect" 20
  Check (Wait-Text $win "Exact content of the note") "its exact preview can be inspected"
  Check ((Note-Files).Count -eq 0) "nothing is written on the model's word"
  Click-Button $win "Approve" 20
  Check (Wait-Files 1) "approving writes exactly one note"
  $content = [System.IO.File]::ReadAllText((Note-Files)[0].FullName, [System.Text.UTF8Encoding]::new($false))
  Check ($content -ceq "# Chat plan`n`nstep 1 from chat`n") "the note holds exactly the approved preview"
  Check ($null -ne (Wait-Contains $win "Written and verified")) "the write step reports the verified file"

  # --- stopping a reply keeps what arrived ---
  Send-Chat $win "Write a long essay"
  Check (Wait-Text $win "Starting a long reply") "partial text is shown while streaming"
  Click-Button $win "Stop"
  Check ($null -ne (Wait-Contains $win "Stopped by you")) "the stopped reply says it was stopped"

  # --- a provider failure is reported, not hidden ---
  Send-Chat $win "Once more"
  Check ($null -ne (Wait-Contains $win "The provider's quota has been reached")) "a quota error is reported"
  Check ($null -ne (Wait-Contains $win "retry after 30 s")) "the retry hint from the provider is kept"

  # --- closing and reopening the window keeps the task ---
  $ui.CloseMainWindow() | Out-Null
  $ui.WaitForExit(10000) | Out-Null
  Check (-not $hostProc.HasExited) "the host survived closing the window"
  $ui = Start-Process $exe -PassThru
  $win = Get-Window $ui.Id
  Check (Wait-Text $win "Nexus.Host") "the reopened window reconnected"
  Click-Button $win "Tasks"
  $row = $null
  $deadline = (Get-Date).AddSeconds(10)
  while (-not $row -and (Get-Date) -lt $deadline) {
    $row = $win.FindAll($S::Descendants, $True_) | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Button -and $_.Current.Name -like "Hello!*" } | Select-Object -First 1
    if (-not $row) { Start-Sleep -Milliseconds 250 }
  }
  Check ($null -ne $row) "the task is listed after reopening"
  if ($row) { Invoke-Element $row }
  Check (Wait-Text $win "Hello! I'm your Nexus assistant.") "its runs are loaded from the host"

  # --- Overview: a task given from the command bar, followed live ---
  Click-Button $win "Overview"
  Check ($null -ne (Wait-Match $win "^\d+%$")) "the system panel shows a real CPU reading"
  Set-Field $win "Give Nexus a task" "Check the private page"
  Click-Button $win "Run" 20
  Check (Wait-Text $win "Done from the dashboard.") "the live panel shows the run's output"
  Check ($null -ne (Wait-Like $win "Read 127.0.0.1:1")) "the web read appears as a step"
  Check ($null -ne (Wait-Contains $win "does not resolve to a public address")) "the refused read says why"
  Check (Wait-Text $win "Live execution") "the live execution panel is shown"
  Check (Wait-Text $win "Execution path") "the core shows the path the run took"
  Check ($null -ne (Wait-Like $win "Web read node")) "the web tool appears as a node around the core"

  # --- Activity: the log names the task of each entry ---
  Click-Button $win "Activity"
  Check ($null -ne (Wait-Like $win "Check the private page")) "the activity log names the task each entry belongs to"
  Check ($null -ne (Wait-Contains $win "Note written and verified")) "host events (the verified write) are in the log"

  # --- a temporary outage is tried again, visibly ---
  Click-Button $win "Overview"
  Set-Field $win "Give Nexus a task" "Try once more"
  Click-Button $win "Run" 20
  Check (Wait-Text $win "Recovered after a retry." 30) "a 503 is tried again and the task still answers"
  Check ($null -ne (Wait-Like $win "try 2")) "the second try is its own step"
  Check ($null -ne (Wait-Contains $win "Temporary outage")) "the failed try says it was a temporary outage"

  # --- projects: a new one becomes the place for new tasks ---
  Click-Button $win "Projects"
  Check (Wait-Text $win 'Active — new tasks go here') "the default project holds the existing tasks and is active"
  Set-Field $win "Name" "Side project"
  Click-Button $win "Create project"
  Check (Wait-Text $win "Side project") "the new project is listed"
  Check ($null -ne (Find-Named $win "Make active" ([System.Windows.Automation.ControlType]::Button))) "the old project can be made active again"
  Check ($null -ne (Find-Named $win "Project for new tasks" ([System.Windows.Automation.ControlType]::ComboBox))) "the command bar offers the project for new tasks"

  # --- memory: the agent proposes, you decide, then it is sent ---
  Set-Field $win "Give Nexus a task" "Remember that I like short answers"
  Click-Button $win "Run" 20
  Check (Wait-Text $win "Noted; I will remember it once you approve." 20) "the reply finishes while the fact waits"
  Check (Wait-Text $win "Remember this? Nothing is kept until you approve") "the live panel asks you to review the fact"
  Check ($null -ne (Wait-Like $win "Propose to remember")) "the proposal is a step of the run"
  Click-Button $win "Memory"
  Check (Wait-Text $win "The user prefers short answers.") "the proposed fact is listed with its source"
  Check ($null -ne (Wait-Like $win "Remember that I like short answers")) "the fact names the task it came from"
  Check ($null -ne (Wait-Contains $win "0 facts · 0 characters")) "nothing is sent before you approve"
  Click-Button $win "Remember"
  Check ($null -ne (Wait-Contains $win "1 fact · 31 characters")) "the approved fact is in the working context"
  Check (-not (Wait-Text $win "Remember this? Nothing is kept until you approve" 2)) "nothing is left to review"
  Click-Button $win "Overview"
  Set-Field $win "Give Nexus a task" "Answer briefly"
  Click-Button $win "Run" 20
  Check (Wait-Text $win "Short answer." 20) "the next task is answered"
  Check ($null -ne (Wait-Like $win "Memory node, 1 fact sent")) "the core shows the approved fact went with the request"

  # --- a fact you add, found by search, then deleted ---
  Click-Button $win "Memory"
  Set-Field $win "Fact" "I write in Romanian."
  Click-Button $win "Remember this"
  Check (Wait-Text $win "Remembered.") "a fact you add is remembered at once"
  Set-Field $win "Search memory" "romanian"
  Check (Wait-Text $win "1 fact") "search finds it by a word"
  Click-Button $win "Delete"
  Click-Button $win "Delete for good"
  Check (Wait-Text $win "No fact matches this search.") "a deleted fact is gone"

  # --- removing the connection removes its key ---
  Click-Button $win "Models"
  Check (Wait-Text $win "No backup model.") "there is no backup model unless one is chosen"
  Click-Button $win "Remove connection"
  Click-Button $win "Confirm removal"
  Check (Wait-Text $win "No connections yet.") "the connection is removed"
  Check ((Vault-Entries).Count -eq 0) "its Credential Manager entry is gone"
  Check ($null -ne (Wait-Contains $win "Nexus sends nothing until you choose a connection and a model")) "nothing can run again; there is no fallback"
}
finally {
  if ($ui -and -not $ui.HasExited) { $ui.CloseMainWindow() | Out-Null; $ui.WaitForExit(10000) | Out-Null }
  if ($hostProc -and -not $hostProc.HasExited) { Stop-Process -Id $hostProc.Id -Force }
  if ($fake -and -not $fake.HasExited) { Stop-Process -Id $fake.Id -Force }
  Start-Sleep -Milliseconds 500
  # Safety net: no test entry may stay in Credential Manager.
  foreach ($t in Vault-Entries) { cmdkey /delete:$t | Out-Null }
  Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue
  Remove-Item Env:NEXUS_PROFILE_DIR -ErrorAction SilentlyContinue
  Remove-Item Env:NEXUS_GEMINI_BASE_URL -ErrorAction SilentlyContinue
}

if ($failures -gt 0) { Write-Output "$failures check(s) failed"; exit 1 }
Write-Output "all checks passed"
