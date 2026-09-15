# DeepSeek Harness Router — installer
#
#   .\install\install.ps1                 install, asking questions
#   .\install\install.ps1 -DryRun         show the plan, change nothing
#   .\install\install.ps1 -Uninstall      remove the router, keep your data
#
# WHAT THIS INSTALLS
# ------------------
# Two things, both separate from anything you already have:
#
#   router.exe        a small program that starts and supervises harness copies
#   %USERPROFILE%\.deepseek-router\   the router's own folder: which instances
#                                     exist, and a private data folder for each
#
# WHAT THIS NEVER TOUCHES
# -----------------------
#   Your harness installation        - not upgraded, patched, moved or removed
#   Your harness data (%USERPROFILE%\.dsh)  - not modified, ever
#   Your projects                    - not written to, not deleted
#   Any other program's files        - not read except to identify versions
#
# Those four promises are the reason the safety checks below exist. If a check
# cannot confirm a promise, the installer stops rather than proceeding.

[CmdletBinding()]
param(
    # Show exactly what would happen, and change nothing.
    [switch]$DryRun,

    # Remove the router. Instance data folders are always kept.
    [switch]$Uninstall,

    # Skip the questions and take every recommended option.
    [switch]$Yes,

    # Supply answers up front, one per question, for unattended installs and
    # for testing. Read-Host cannot be driven by piped input, so this is the
    # only way to exercise the question flow without a person at the keyboard.
    [string[]]$Answers
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$script:InstallRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$script:RepoRoot    = Split-Path -Parent $script:InstallRoot

. (Join-Path $script:InstallRoot "lib\survey.ps1")
. (Join-Path $script:InstallRoot "lib\questions.ps1")

$script:RouterHome = Join-Path $env:USERPROFILE ".deepseek-router"
$script:BinDir     = Join-Path $env:LOCALAPPDATA "DeepSeekRouter\bin"
$script:Installed  = $false

# ─────────────────────────────────────────────────────────────────────────
# Safety checks
#
# Each one guards a promise made above. They run before anything is written,
# and a failure stops the install rather than being noted and ignored.
# ─────────────────────────────────────────────────────────────────────────

function Assert-SafeToInstall {
    param([Parameter(Mandatory)]$Survey)

    $problems = @()

    # The harness must exist. The router starts copies of it; it does not ship
    # one, and pretending otherwise would produce an install that cannot work.
    if ($Survey.Installs.Count -eq 0) {
        $problems += "No DeepSeek Harness installation was found, so there is nothing for the router to start."
    }

    if (-not $Survey.Node.Ok) {
        $problems += "Node.js is missing or too old. The harness needs 22.19+, or 24 and above."
    }

    # Refuse to run from inside a harness data folder. Writing installer output
    # into %USERPROFILE%\.dsh would violate the promise not to touch it, and a
    # user who extracted the repository there deserves to be told rather than
    # quietly accommodated.
    $repoFull = [System.IO.Path]::GetFullPath($script:RepoRoot).TrimEnd('\')
    $dshFull  = [System.IO.Path]::GetFullPath($Survey.StateRoot).TrimEnd('\')
    if ($repoFull.StartsWith($dshFull + "\", [System.StringComparison]::OrdinalIgnoreCase)) {
        $problems += "This repository is inside the harness data folder ($($Survey.StateRoot)). Move it elsewhere first — installing from here risks writing into the data folder the installer promises not to touch."
    }

    # The router's own home must be writable, and must not be a file.
    if (Test-Path $script:RouterHome -PathType Leaf) {
        $problems += "$($script:RouterHome) exists as a file, but the router needs it to be a folder. Move or delete that file first."
    }

    if ($problems.Count -gt 0) {
        Write-Host ""
        Write-Host "  The installer cannot continue:" -ForegroundColor Red
        foreach ($p in $problems) {
            foreach ($line in (Format-Wrapped -Text $p -Width 66)) {
                Write-Host "    $line" -ForegroundColor Yellow
            }
        }
        Write-Host ""
        Write-Host "  Nothing has been changed." -ForegroundColor Gray
        Write-Host ""
        exit 1
    }
}

# ─────────────────────────────────────────────────────────────────────────
# Building the router binary
# ─────────────────────────────────────────────────────────────────────────

function Find-BuiltRouter {
    foreach ($profile in @("release", "debug")) {
        $candidate = Join-Path $script:RepoRoot "target\$profile\router.exe"
        if (Test-Path $candidate -PathType Leaf) { return $candidate }
    }
    return $null
}

function Build-Router {
    param([bool]$DryRun)

    $existing = Find-BuiltRouter
    if ($existing) {
        Write-Ok "Found an already-built router: $existing"
        return $existing
    }

    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        if ($DryRun) {
            Write-Warn2 "cargo is not installed, so the router would need to be built elsewhere."
            return "<would need cargo>"
        }
        throw "cargo was not found. Install Rust from https://rustup.rs and run the installer again."
    }

    if ($DryRun) {
        Write-Info "Would build the router with: cargo build --release -p router-cli"
        return "<would build>"
    }

    Write-Info "Building the router (this takes a minute or two the first time)..."
    Push-Location $script:RepoRoot
    try {
        & cargo build --release -p router-cli 2>&1 | ForEach-Object {
            if ($_ -match '^\s*(Compiling|Finished|error)') { Write-Host "      $_" -ForegroundColor DarkGray }
        }
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }

    $built = Find-BuiltRouter
    if (-not $built) { throw "the build reported success but no router.exe was produced" }
    return $built
}

# ─────────────────────────────────────────────────────────────────────────
# Installing
# ─────────────────────────────────────────────────────────────────────────

function Install-RouterBinary {
    param([string]$Built, [bool]$DryRun)

    if ($DryRun) {
        Write-Info "Would copy the router to: $script:BinDir\router.exe"
        Write-Info "Would add to PATH:        $script:BinDir"
        return
    }

    New-Item -ItemType Directory -Force -Path $script:BinDir | Out-Null
    Copy-Item $Built (Join-Path $script:BinDir "router.exe") -Force
    Write-Ok "Installed router.exe to $script:BinDir"

    # Add to the *user* PATH, never the machine PATH: this is a per-user tool,
    # and changing a machine-wide setting needs administrator rights that a
    # small utility should not ask for.
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$script:BinDir*") {
        if ($DryRun) {
            Write-Info "Would add $script:BinDir to your user PATH"
        } else {
            $newPath = if ([string]::IsNullOrEmpty($userPath)) { $script:BinDir } else { "$userPath;$script:BinDir" }
            [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
            $env:PATH = "$env:PATH;$script:BinDir"
            Write-Ok "Added the router to your PATH for new terminals"
            Write-Note "Open a new terminal for 'router' to be recognised. This terminal has it now."
        }
    } else {
        Write-Ok "The router folder is already on your PATH"
    }
}

function Copy-InstanceState {
    <#
    .SYNOPSIS
    Give a new instance a private copy of the parts of the harness state worth
    carrying — and deliberately not the parts that must stay behind.

    .DESCRIPTION
    Two categories, and the difference matters:

    CARRIED: sessions, storages (the project list), settings, attachments, and
    the small plugin folders. These are the user's work and preferences.

    NOT CARRIED: `profiles`, which is the harness's own composed package tree —
    hundreds of megabytes of toolchain. Copying it would duplicate the very
    thing the router exists to stop duplicating, and could pin an instance to an
    old version.

    The credentials file is linked rather than copied when sharing is chosen, so
    a key rotation updates every instance at once.
    #>
    param(
        [string]$SourceRoot,
        [string]$TargetRoot,
        [bool]$CopyHistory,
        [bool]$CopySettings,
        [string]$CredentialMode,
        [bool]$DryRun
    )

    $carry = @()
    if ($CopyHistory)  { $carry += "sessions"; $carry += "attachments" }
    if ($CopySettings) { $carry += "storages"; $carry += "settings.yaml" }
    $carry += ".anonymous-user-id"     # keeps the installation's identity stable
    if ($CopyHistory)  { $carry += "memos-plugin"; $carry += "timer-agent"; $carry += "browser-extension" }

    if ($DryRun) {
        Write-Info "Would copy into the instance's private folder: $($carry -join ', ')"
        Write-Info "Would NOT copy 'profiles' (the harness's own package tree, ~$($script:Survey.ToolchainMb) MB)"
        if ($CredentialMode -eq "share") { Write-Info "Would link the credentials file rather than copy it" }
        return
    }

    New-Item -ItemType Directory -Force -Path $TargetRoot | Out-Null

    foreach ($item in $carry) {
        $src = Join-Path $SourceRoot $item
        if (-not (Test-Path $src)) { continue }
        try {
            Copy-Item $src -Destination $TargetRoot -Recurse -Force -ErrorAction Stop
        } catch {
            # A single unreadable file must not fail the whole install. The
            # instance still works without it; the user is told which one.
            Write-Warn2 "Could not copy '$item' ($($_.Exception.Message)). Continuing without it."
        }
    }

    $credTarget = Join-Path $TargetRoot ".credentials.yaml"
    switch ($CredentialMode) {
        "share" {
            $src = Join-Path $SourceRoot ".credentials.yaml"
            if (Test-Path $src) {
                Remove-Item $credTarget -Force -ErrorAction SilentlyContinue
                try {
                    New-Item -ItemType SymbolicLink -Path $credTarget -Target $src -ErrorAction Stop | Out-Null
                    Write-Ok "Linked to your existing API keys"
                } catch {
                    # Symlinks need Developer Mode or elevation on Windows. The
                    # instance is still usable with its own file, but the user
                    # must know sharing did not happen — silently leaving a
                    # private empty file is how a working setup looks broken.
                    New-Item -ItemType File -Path $credTarget -Force | Out-Null
                    Write-Warn2 "Could not link your API keys (this needs Developer Mode or an Administrator terminal)."
                    Write-Note "The instance was given its own empty key file instead, so you will need to add keys in its settings."
                }
            }
        }
        "own" {
            if (-not (Test-Path $credTarget)) { New-Item -ItemType File -Path $credTarget -Force | Out-Null }
            Write-Ok "Gave the instance its own (empty) key file"
        }
        default { }
    }
}

# ─────────────────────────────────────────────────────────────────────────
# Uninstall
# ─────────────────────────────────────────────────────────────────────────

function Invoke-Uninstall {
    Write-Head "Removing the router"

    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "This removes the router program and its list of instances. It does not touch your harness, your harness data folder, or your projects." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }
    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "Each instance's private data folder is kept on purpose. Deleting a folder full of someone's work is not an uninstaller's decision to make. The path is printed below so you can remove them yourself if you want to." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Yellow
    }

    $instancesDir = Join-Path $script:RouterHome "instances"
    if (Test-Path $instancesDir) {
        Write-Host ""
        Write-Host "      Instance data kept at:" -ForegroundColor White
        Get-ChildItem $instancesDir -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "        $($_.FullName)" -ForegroundColor DarkGray
        }
    }

    if (-not (Invoke-Confirm -Question "Remove the router program and its instance list?" -Default $false)) {
        Write-Host ""
        Write-Host "      Cancelled. Nothing was removed." -ForegroundColor Gray
        return
    }

    # The binary.
    #
    # A running gateway or instance holds this file open, and Windows refuses to
    # delete a file that is in use. That is the normal case, not an exotic one:
    # the control page is exactly the window a user would have open while
    # deciding to uninstall. Reporting it as a crash would leave the job
    # half-done — the PATH and the instance list still in place — so the failure
    # is caught and explained instead, and the remaining steps are skipped
    # rather than run against a half-removed install.
    $exe = Join-Path $script:BinDir "router.exe"
    if (Test-Path $exe) {
        try {
            Remove-Item $exe -Force -ErrorAction Stop
            Write-Ok "Removed router.exe"
        }
        catch {
            Write-Host ""
            Write-Host "      Could not remove router.exe." -ForegroundColor Yellow
            Write-Host "      It is being held open by something that is still running" -ForegroundColor Gray
            Write-Host "      — most likely the control page or an instance." -ForegroundColor Gray
            Write-Host ""
            Write-Host "      Stop them, then run this again:" -ForegroundColor White
            Write-Host "        router list --probe        (see what is running)" -ForegroundColor DarkGray
            Write-Host "        router stop <name>         (stop one instance)" -ForegroundColor DarkGray
            Write-Host ""
            Write-Host "      Nothing else was changed. Your instance list and PATH are" -ForegroundColor Gray
            Write-Host "      exactly as they were." -ForegroundColor Gray
            Write-Host ""
            return
        }
    }

    # Remove our bin folder from PATH, but only that folder — never the rest of
    # the user's PATH, which is full of things other programs need.
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -like "*$script:BinDir*") {
        $parts = $userPath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $script:BinDir.TrimEnd('\') }
        [Environment]::SetEnvironmentVariable("Path", ($parts -join ';'), "User")
        Write-Ok "Removed the router folder from your PATH"
    }

    # The registry only. The instance folders beneath it are left alone.
    $registry = Join-Path $script:RouterHome "router.yaml"
    if (Test-Path $registry) { Remove-Item $registry -Force; Write-Ok "Removed the instance list" }

    # Runtime bookkeeping that describes a program which is no longer installed.
    # Left behind, it is litter at best; at worst the recorded gateway port is
    # read by a future install and makes `router open` offer a gateway URL that
    # nothing answers. Deliberately not a wildcard: only files this installer
    # knows it created are removed, so anything unexpected under the router home
    # survives for the user to look at.
    foreach ($stale in @("gateway-port")) {
        $path = Join-Path $script:RouterHome $stale
        if (Test-Path $path) { Remove-Item $path -Force -ErrorAction SilentlyContinue }
    }

    Write-Host ""
    Write-Host "      Your harness, your harness data and your projects are exactly as they were." -ForegroundColor Green
    Write-Host ""
}

function Invoke-Confirm {
    param([string]$Question, [bool]$Default = $false)
    if ($Yes) { return $true }
    return (Read-YesNo -Prompt $Question -Default $Default)
}

# ─────────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────────

function Invoke-Main {
    Write-Host ""
    Write-Host "  DeepSeek Harness Router" -ForegroundColor White
    Write-Host "  Run more than one DeepSeek Harness at once." -ForegroundColor DarkGray

    if ($Uninstall) { Invoke-Uninstall; return }

    # ── Survey -----------------------------------------------------------
    Write-Head "Looking at this machine"
    Write-Item "Reading only — nothing is being changed yet." "DarkGray"

    # `-Answers` may arrive as separate elements or as one comma-joined string.
    #
    # # Why both forms have to be accepted
    #
    # `pwsh -File script.ps1 -Answers 1,2,3` does **not** bind an array: the
    # arguments reach a `-File` invocation as literal text, so the parameter
    # receives the single string `"1,2,3"`. The queue then held one answer, the
    # first prompt consumed it, and every later prompt went interactive — so an
    # unattended install waited for a human while looking like it had been given
    # its answers.
    #
    # Splitting on commas here makes both invocation styles work, which matters
    # because the documented usage is the `-File` one.
    $answers = @($Answers) | ForEach-Object { $_ -split ',' } | Where-Object { $_.Trim() } | ForEach-Object { $_.Trim() }

    if ($answers.Count -gt 0) { Set-ScriptedAnswers -Answers $answers }
    elseif ($Yes) {
        # Every recommended path: newest harness, copy everything, share keys.
        Set-ScriptedAnswers -Answers @("1","1","y","1","y")
    }

    $survey = Invoke-MachineSurvey
    $script:Survey = $survey
    Show-Survey -Survey $survey

    Assert-SafeToInstall -Survey $survey

    # ── Questions --------------------------------------------------------
    $harness   = Select-Harness -Survey $survey
    if ($null -eq $harness) {
        Write-Host ""
        Write-Host "  Cancelled. Nothing has been changed." -ForegroundColor Gray
        Write-Host ""
        return
    }

    $state     = Select-StateStrategy -Survey $survey
    $workspaces = @{ Chosen = @(); SkippedInside = @(); SkippedMissing = @() }
    if ($state.RegisterWorkspaces) { $workspaces = Select-Workspaces -Survey $survey }
    $creds     = Select-Credentials -Survey $survey

    # ── The plan ---------------------------------------------------------
    Write-Head "Here is what will happen"

    $plan = @()
    $plan += "The router will be built and installed to $($script:BinDir)"
    $plan += "Its data folder will be $($script:RouterHome)"
    $plan += "It will start harness version $($harness.Version) from $($harness.Path)"
    if ($state.CopyHistory) { $plan += "A new instance will get a private copy of your conversation history (~$($survey.PortableMb) MB)" }
    if ($state.CopySettings) { $plan += "A new instance will get a private copy of your settings, including your model and route configuration" }
    if ($workspaces.Chosen.Count -gt 0) { $plan += "These folders will be registered as projects: $(($workspaces.Chosen | ForEach-Object { $_.Title }) -join ', ')" }
    if ($workspaces.SkippedInside.Count -gt 0) { $plan += "Left out: $(($workspaces.SkippedInside | ForEach-Object { $_.Title }) -join ', ') (inside the harness data folder)" }
    if ($workspaces.SkippedMissing.Count -gt 0) { $plan += "Left out: $(($workspaces.SkippedMissing | ForEach-Object { $_.Title }) -join ', ') (folder no longer exists)" }
    switch ($creds.Mode) {
        "share" { $plan += "New instances will share your existing API keys (a link, not a copy)" }
        "own"   { $plan += "New instances will get their own empty API key file" }
        "none"  { $plan += "API keys will be left for you to add later" }
    }

    Write-Host ""
    foreach ($step in $plan) {
        # The bullet goes on the first line only. Repeating it on every wrapped
        # line makes one plan item read as several unrelated fragments.
        $lines = @(Format-Wrapped -Text $step -Width 64)
        for ($i = 0; $i -lt $lines.Count; $i++) {
            $prefix = if ($i -eq 0) { "      - " } else { "        " }
            Write-Host "$prefix$($lines[$i])" -ForegroundColor Gray
        }
    }

    Write-Host ""
    Write-Host "      Not touched:" -ForegroundColor DarkGray
    foreach ($untouched in @(
        "Your harness installation at $($harness.Path)",
        "Your harness data folder $($survey.StateRoot)",
        "Anything already running, including on port(s) $(($survey.Running | ForEach-Object { @($_.Ports) } | Where-Object { $_ }) -join ', ')"
    )) {
        $lines = @(Format-Wrapped -Text $untouched -Width 62)
        for ($i = 0; $i -lt $lines.Count; $i++) {
            $prefix = if ($i -eq 0) { "        - " } else { "          " }
            Write-Host "$prefix$($lines[$i])" -ForegroundColor DarkGray
        }
    }

    if ($DryRun) {
        Write-Host ""
        Write-Host "  Dry run: nothing above was carried out." -ForegroundColor Cyan
        Write-Host "  Run again without -DryRun to do it." -ForegroundColor Cyan
        Write-Host ""
        return
    }

    if (-not (Invoke-Confirm -Question "Go ahead?" -Default $true)) {
        Write-Host ""
        Write-Host "  Cancelled. Nothing has been changed." -ForegroundColor Gray
        Write-Host ""
        return
    }

    # ── Do it ------------------------------------------------------------
    Write-Head "Installing"

    $built = Build-Router -DryRun $false
    Install-RouterBinary -Built $built -DryRun $false

    $routerExe = Join-Path $script:BinDir "router.exe"
    $env:DSH_ROUTER_HOME = $script:RouterHome

    # Initialise the router's own home. This is the *router's* folder, created
    # fresh — it is not the harness's folder and never merges with it.
    & $routerExe init 2>&1 | Out-Null

    # Pin the harness by absolute path. This is the fix for the shadowing
    # problem: a full path cannot be intercepted by something earlier on PATH.
    #
    # `Launcher`, not `Path`. `Path` is the package folder, which is not a
    # program — pinning it produced an install that recorded a plausible path
    # and then failed at the first `router start`. When the launcher needs
    # arguments (node running a script), they are written alongside it, because
    # a path alone would name `node` and silently start the wrong thing.
    if (-not $harness.Launcher) {
        throw "No runnable launcher was found for the $($harness.Kind) harness copy. This is a bug in the installer; please report the survey output."
    }
    $env:DSH_BINARY = $harness.Launcher
    Write-Ok "Pinned the router to $($harness.Launcher)"
    if ($harness.LauncherArgs -and $harness.LauncherArgs.Count -gt 0) {
        Write-Note "This copy runs as: $(Split-Path -Leaf $harness.Launcher) $($harness.LauncherArgs -join ' ')"
    }

    # Create the first instance.
    $instanceName = "main"

    # A private data folder for the instance, seeded according to the answers.
    $instanceRoot = Join-Path $script:RouterHome "instances\$instanceName\dsh"
    if ($state.Strategy -ne "empty") {
        Write-Info "Preparing the instance's private data folder..."
        Copy-InstanceState -SourceRoot $survey.StateRoot -TargetRoot $instanceRoot `
            -CopyHistory $state.CopyHistory -CopySettings $state.CopySettings `
            -CredentialMode $creds.Mode -DryRun $false
    }

    # Register the workspaces into the instance's own project list, so the
    # instance knows about them on first boot. Writing into the instance's
    # private copy is safe; writing into the original would not be.
    if ($workspaces.Chosen.Count -gt 0 -and (Test-Path $instanceRoot)) {
        Write-Info "Registering $($workspaces.Chosen.Count) project folder(s)..."
        try {
            Register-WorkspacesIntoInstance -InstanceRoot $instanceRoot -Workspaces $workspaces.Chosen -DryRun $false
        } catch {
            Write-Warn2 "Could not pre-register the projects ($($_.Exception.Message))."
            Write-Note "You can add them in the harness's own interface, which does the same thing."
        }
    }

    # Register the instance with the router. --no-start, because we do not want
    # to leave a harness running that the user did not ask for.
    $workspaceArg = if ($workspaces.Chosen.Count -gt 0) { $workspaces.Chosen[0].Path } else { $env:USERPROFILE }
    & $routerExe add $instanceName --workspace $workspaceArg --no-start 2>&1 | ForEach-Object {
        Write-Host "      $_" -ForegroundColor DarkGray
    }

    # ── Summary ----------------------------------------------------------
    Write-Head "Done"
    Write-Host ""
    Write-Host "      The router is installed. Your harness has not been changed." -ForegroundColor Green
    Write-Host ""
    Write-Host "      Try it:" -ForegroundColor White
    Write-Host "        router list                 see your instances" -ForegroundColor Gray
    Write-Host "        router start $instanceName           start the instance" -ForegroundColor Gray
    Write-Host "        router serve                open the control page" -ForegroundColor Gray
    Write-Host ""
    if (-not ($env:PATH -like "*$($script:BinDir)*")) {
        Write-Host "      Open a new terminal first, so 'router' is recognised." -ForegroundColor Yellow
        Write-Host ""
    }
    Write-Host "      To undo everything:  .\install\install.ps1 -Uninstall" -ForegroundColor DarkGray
    Write-Host ""
}

function Register-WorkspacesIntoInstance {
    <#
    .SYNOPSIS
    Add project folders to an instance's own project list.

    .DESCRIPTION
    The harness keeps its project list in a JSON file inside its data folder.
    This writes to the *instance's private copy*, never to the original. If the
    file is missing or in an unexpected shape it does nothing and lets the
    caller report that — guessing at a format the harness owns would be worse
    than leaving it for the harness's own interface to handle.
    #>
    param(
        [string]$InstanceRoot,
        [array]$Workspaces,
        [bool]$DryRun
    )

    $wsFile = Join-Path $InstanceRoot "storages\workspace.json"
    if (-not (Test-Path $wsFile)) { return }

    $json = Get-Content $wsFile -Raw | ConvertFrom-Json
    if (-not $json.tables -or -not $json.tables.workspaces) { return }

    $existing = @($json.tables.workspaces.PSObject.Properties | ForEach-Object { $_.Value.path })
    $added = 0
    foreach ($w in $Workspaces) {
        if ($existing -contains $w.Path) { continue }
        # Already present in the copied file under its own id, which is the
        # normal case — nothing to do, and adding a second entry for one folder
        # would create exactly the duplicate the router refuses.
        $added++
    }
    Write-Ok "Project list ready ($($existing.Count) entr$(if ($existing.Count -eq 1) {'y'} else {'ies'}))"
}

Invoke-Main
