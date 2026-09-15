# DeepSeek Harness Router — machine survey
#
# WHY THIS FILE EXISTS
# --------------------
# Installing the router means answering questions about a machine we have never
# seen: where the harness is, which copy of it runs, whether something is
# already listening, and whether there are existing projects to bring along.
#
# Every one of those answers can be wrong in a way that silently produces a
# broken setup. So the installer does not assume. It looks, reports what it
# found, and explains what each finding means before offering a choice.
#
# The survey is READ-ONLY. It starts no processes, writes no files, and changes
# nothing. Running it is always safe.

Set-StrictMode -Version Latest

# ─────────────────────────────────────────────────────────────────────────
# Output helpers
#
# The installer's whole job is to be understood, so presentation is not
# decoration here — it is the interface. Three levels, used consistently:
# a heading introduces a subject, an item is a finding, and a note explains
# why the finding matters.
# ─────────────────────────────────────────────────────────────────────────

function Write-Head {
    param([string]$Text)
    Write-Host ""
    Write-Host "  $Text" -ForegroundColor White
    Write-Host ("  " + ("─" * [Math]::Min($Text.Length + 8, 74))) -ForegroundColor DarkGray
}

function Write-Item {
    param([string]$Text, [string]$Colour = "Gray")
    Write-Host "    $Text" -ForegroundColor $Colour
}

function Write-Note {
    param([string]$Text)
    # Wrapped by hand rather than by the console, because a note is prose and
    # the terminal's own wrapping would break the indentation that separates a
    # finding from its explanation.
    foreach ($line in (Format-Wrapped -Text $Text -Width 68)) {
        Write-Host "      $line" -ForegroundColor DarkGray
    }
}

function Format-Wrapped {
    param([string]$Text, [int]$Width = 68)
    $words = $Text -split '\s+'
    $lines = @()
    $current = ""
    foreach ($w in $words) {
        if ($current.Length -eq 0) { $current = $w }
        elseif (($current.Length + 1 + $w.Length) -le $Width) { $current += " $w" }
        else { $lines += $current; $current = $w }
    }
    if ($current.Length -gt 0) { $lines += $current }
    return $lines
}

function Write-Ok    { param([string]$T) Write-Host "    [ok]   $T" -ForegroundColor Green }
function Write-Warn2 { param([string]$T) Write-Host "    [warn] $T" -ForegroundColor Yellow }
function Write-Bad   { param([string]$T) Write-Host "    [stop] $T" -ForegroundColor Red }
function Write-Info  { param([string]$T) Write-Host "    [info] $T" -ForegroundColor Cyan }

# ─────────────────────────────────────────────────────────────────────────
# Small utilities
# ─────────────────────────────────────────────────────────────────────────

function Get-VersionFromPackage {
    param([string]$PackageJsonPath)
    if (-not (Test-Path $PackageJsonPath)) { return $null }
    try {
        $j = Get-Content $PackageJsonPath -Raw -ErrorAction Stop | ConvertFrom-Json
        return $j.version
    } catch {
        # An unparseable package.json is a broken install, not a crash for us.
        return $null
    }
}

function Compare-SemverLike {
    param([string]$A, [string]$B)
    # Enough ordering for "which of these two is newer". Pre-release suffixes
    # are compared as strings, which is wrong in general but adequate for the
    # harness's own `0.1.x-rc.n` scheme, and the caller only ever uses this to
    # decide which copy to *prefer* — never to reject one outright.
    if (-not $A) { return -1 }
    if (-not $B) { return 1 }
    $na = ($A -replace '[^0-9.].*$','') -split '\.' | ForEach-Object { [int]($_ + '0' -replace '\D','') }
    $nb = ($B -replace '[^0-9.].*$','') -split '\.' | ForEach-Object { [int]($_ + '0' -replace '\D','') }
    for ($i = 0; $i -lt [Math]::Max($na.Count, $nb.Count); $i++) {
        $x = if ($i -lt $na.Count) { $na[$i] } else { 0 }
        $y = if ($i -lt $nb.Count) { $nb[$i] } else { 0 }
        if ($x -ne $y) { return ($x - $y) }
    }
    return 0
}

function Test-PortListening {
    param([int]$Port)
    return [bool](Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue)
}

function Get-ListenerOwner {
    param([int]$Port)
    $c = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $c) { return $null }
    $p = Get-CimInstance Win32_Process -Filter "ProcessId=$($c.OwningProcess)" -ErrorAction SilentlyContinue
    return [pscustomobject]@{
        Pid         = $c.OwningProcess
        Name        = if ($p) { $p.Name } else { "unknown" }
        CommandLine = if ($p) { $p.CommandLine } else { "" }
    }
}

# ─────────────────────────────────────────────────────────────────────────
# The survey
# ─────────────────────────────────────────────────────────────────────────

function Invoke-MachineSurvey {
    <#
    .SYNOPSIS
    Look at this machine and report what is actually here.

    .DESCRIPTION
    Returns an object describing every fact the installer needs, plus a list of
    findings. Each finding carries a severity, a plain-language explanation, and
    what it means for the install. Nothing here writes or starts anything.

    The findings are the point. A machine can look fine and still be set up in a
    way that breaks later — two harness copies where the older one shadows the
    newer, a workspace living inside the state directory, a state root already
    in use. Those are the situations this exists to catch.
    #>

    $f = @()   # findings

    # ── 1. Node ──────────────────────────────────────────────────────────
    $node = $null
    try { $node = (& node --version 2>$null) } catch { $node = $null }

    $nodeOk = $false
    if ($node) {
        $v = $node -replace '^v',''
        $major = [int](($v -split '\.')[0])
        $minor = [int](($v -split '\.')[1])
        # The harness documents ^22.19.0 || >=24.0.0
        $nodeOk = ($major -eq 22 -and $minor -ge 19) -or ($major -ge 24)
    }

    # ── 2. Every harness on PATH, in resolution order ────────────────────
    #
    # Order matters more than presence. The first `dsh` on PATH is the one a
    # user's shell runs, and a second copy earlier in PATH silently shadows a
    # later one. That is not hypothetical: it is what happened on the machine
    # this installer was first written for, and it broke `dsh web` for days.
    $onPath = @()
    $index = 0
    foreach ($dir in ($env:PATH -split ';')) {
        $index++
        if (-not $dir) { continue }
        foreach ($name in @("dsh.cmd","dsh.exe","dsh.bat","dsh")) {
            $candidate = Join-Path $dir $name
            if (Test-Path $candidate -PathType Leaf) {
                $onPath += [pscustomobject]@{
                    PathIndex = $index
                    Path      = $candidate
                    Dir       = $dir
                    Name      = $name
                }
                break
            }
        }
    }

    # ── 3. Every harness *install*, wherever it lives ────────────────────
    #
    # Not just PATH. A harness can be installed somewhere unreachable from the
    # shell but still bootable, and the interesting question is which copy is
    # actually going to run.
    $installs = @()
    $candidates = @(
        @{ Kind = "npm global";    Base = (Join-Path $env:APPDATA "npm\node_modules") },
        @{ Kind = "composed profile"; Base = (Join-Path $env:USERPROFILE ".dsh\profiles\node_modules") }
    )
    # Toolchain-style installs: <base>/<anything>/node_modules
    foreach ($toolRoot in @(
        (Join-Path $env:LOCALAPPDATA "OpenDesign\toolchains\dsh"),
        (Join-Path $env:LOCALAPPDATA "Programs")
    )) {
        if (Test-Path $toolRoot) {
            Get-ChildItem $toolRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object {
                $candidates += @{ Kind = "bundled toolchain"; Base = (Join-Path $_.FullName "node_modules") }
            }
        }
    }

    foreach ($c in $candidates) {
        $pkgPath = Join-Path $c.Base "@deepseek-ai\dsh"
        $pkg = Join-Path $pkgPath "package.json"
        if (Test-Path $pkg) {
            # What the router needs is something it can *execute*.
            #
            # Three shapes exist, and only the first is directly runnable:
            #
            #   1. An npm bin shim  - `%APPDATA%\npm\dsh.cmd`, a real executable
            #      wrapper. This is the right answer when it exists.
            #   2. A `.cmd`/`.exe` beside the package.
            #   3. `lib/bin.js`     - a script. Rust's `Command::new` cannot run
            #      a `.js` on Windows; it needs `node` in front of it.
            #
            # Naming the package *folder* — the first version of this installer
            # did exactly that — produces an install that records a perfectly
            # plausible path and then fails at the first `router start`.
            $launcher = $null
            $launcherArgs = @()

            # 1. The npm shim — but ONLY for the install it actually belongs to.
            #
            # `%APPDATA%\npm\dsh.cmd` is a generated wrapper that runs
            # `npm\node_modules\@deepseek-ai\dsh\lib\bin.js`. It is the right
            # launcher for that package and the *wrong* one for every other
            # copy: pointing the "bundled toolchain" option at it would run the
            # global harness while claiming to run the toolchain's, which is the
            # exact confusion this installer exists to remove.
            if ($c.Kind -eq "npm global") {
                $npmDir = Join-Path $env:APPDATA "npm"
                $shimName = if ($env:OS -eq "Windows_NT") { "dsh.cmd" } else { "dsh" }
                $cand = Join-Path $npmDir $shimName
                if (Test-Path $cand -PathType Leaf) {
                    # The generated shim resolves its script as `%dp0%` plus a
                    # relative path — `%dp0%\node_modules\@deepseek-ai\dsh\
                    # lib\bin.js`, where `%dp0%` is the shim's own directory.
                    # So the honest check is whether that relative path lands on
                    # this package, not whether the shim's text mentions an
                    # absolute path (it never does).
                    $shimText = Get-Content $cand -Raw -ErrorAction SilentlyContinue
                    $rel = "node_modules\@deepseek-ai\dsh"
                    $resolvesHere = $false
                    if ($shimText) {
                        $resolvesHere = $shimText -match [regex]::Escape($rel)
                    }
                    # And confirm it by falling back to the directory test: the
                    # package this shim would run is the one directly under it.
                    $shimTarget = Join-Path $npmDir $rel
                    $samePackage = $false
                    try {
                        $samePackage = ([System.IO.Path]::GetFullPath($shimTarget).TrimEnd('\') -eq
                                        [System.IO.Path]::GetFullPath($pkgPath).TrimEnd('\'))
                    } catch { }

                    if ($resolvesHere -and $samePackage) { $launcher = $cand }
                }
            }

            # 2. An executable inside the package.
            if (-not $launcher) {
                foreach ($cand in @(
                    (Join-Path $pkgPath "bin\dsh.cmd"),
                    (Join-Path $pkgPath "bin\dsh")
                )) {
                    if (Test-Path $cand -PathType Leaf) { $launcher = $cand; break }
                }
            }

            # 3. The script, with node named explicitly.
            if (-not $launcher) {
                $script = Join-Path $pkgPath "lib\bin.js"
                if (Test-Path $script -PathType Leaf) {
                    $nodeExe = (Get-Command node -ErrorAction SilentlyContinue).Source
                    if ($nodeExe) {
                        $launcher = $nodeExe
                        $launcherArgs = @($script)
                    }
                }
            }

            $installs += [pscustomobject]@{
                Kind         = $c.Kind
                Path         = $pkgPath
                Launcher     = $launcher
                LauncherArgs = $launcherArgs
                Version      = (Get-VersionFromPackage $pkg)
            }
        }
    }

    # ── 4. What `dsh --version` actually answers ─────────────────────────
    #
    # This is the trap. A launcher script can route different arguments to
    # different harnesses, so `--version` and `web` can report and run
    # different versions. When that happens, a diagnostic that trusts
    # `--version` describes a program the user is not running.
    $shimVersion = $null
    $shimPath = $null
    try {
        $shimPath = (Get-Command dsh -ErrorAction Stop).Source
        $shimVersion = (& dsh --version 2>$null | Select-Object -First 1)
    } catch { }

    $directVersion = $null
    $directPath = $null
    $npmShim = Join-Path $env:APPDATA "npm\dsh.cmd"
    if (Test-Path $npmShim) {
        $directPath = $npmShim
        try { $directVersion = (& $npmShim --version 2>$null | Select-Object -First 1) } catch { }
    }

    # ── 5. Existing state roots ──────────────────────────────────────────
    $dshHome = Join-Path $env:USERPROFILE ".dsh"
    $stateRootExists = Test-Path $dshHome

    $workspaces = @()
    $workspaceJson = Join-Path $dshHome "storages\workspace.json"
    if (Test-Path $workspaceJson) {
        try {
            $j = Get-Content $workspaceJson -Raw -ErrorAction Stop | ConvertFrom-Json
            if ($j.tables -and $j.tables.workspaces) {
                foreach ($prop in $j.tables.workspaces.PSObject.Properties) {
                    $ws = $prop.Value
                    # A workspace inside the state root is a special hazard: it
                    # means a second instance pointed at it would write into the
                    # state directory itself.
                    $inside = $false
                    if ($ws.path) {
                        $inside = $ws.path.TrimEnd('\') -like "$($dshHome.TrimEnd('\'))\*"
                    }
                    $workspaces += [pscustomobject]@{
                        Id           = $prop.Name
                        Title        = $ws.title
                        Path         = $ws.path
                        SessionCount = @($ws.sessionIds).Count
                        Exists       = if ($ws.path) { Test-Path $ws.path } else { $false }
                        InsideState  = $inside
                    }
                }
            }
        } catch { }
    }

    $sessionCount = 0
    $sessionsDir = Join-Path $dshHome "sessions"
    if (Test-Path $sessionsDir) {
        $sessionCount = @(Get-ChildItem $sessionsDir -ErrorAction SilentlyContinue).Count
    }

    # Size of the parts worth carrying, and the part that must NOT be carried.
    # `profiles` is the harness's own composed package tree — copying it would
    # duplicate 350+ MB of toolchain, which is the opposite of isolation.
    function Get-DirMb {
        param([string]$Path)
        if (-not (Test-Path $Path)) { return 0 }
        $sum = (Get-ChildItem $Path -Recurse -File -ErrorAction SilentlyContinue |
                Measure-Object -Property Length -Sum).Sum
        if (-not $sum) { return 0 }
        return [math]::Round($sum / 1MB, 1)
    }
    $portableMb = (Get-DirMb (Join-Path $dshHome "sessions")) +
                  (Get-DirMb (Join-Path $dshHome "storages")) +
                  (Get-DirMb (Join-Path $dshHome "attachments")) +
                  (Get-DirMb (Join-Path $dshHome "memos-plugin")) +
                  (Get-DirMb (Join-Path $dshHome "browser-extension"))
    $toolchainMb = Get-DirMb (Join-Path $dshHome "profiles")

    # ── 6. Anything already listening where we would start ───────────────
    $running = @()
    Get-CimInstance Win32_Process -Filter "Name='node.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -match 'dsh' -and $_.CommandLine -match '\bweb\b' } |
        ForEach-Object {
            $ports = @(Get-NetTCPConnection -OwningProcess $_.ProcessId -State Listen -ErrorAction SilentlyContinue |
                       Select-Object -ExpandProperty LocalPort)
            $running += [pscustomobject]@{
                Pid         = $_.ProcessId
                Ports       = $ports
                CommandLine = $_.CommandLine
            }
        }

    # ── 7. A previous router install ─────────────────────────────────────
    $routerHome = Join-Path $env:USERPROFILE ".deepseek-router"
    $routerInstalled = Test-Path (Join-Path $routerHome "router.yaml")

    # ═════════════════════════════════════════════════════════════════════
    # FINDINGS
    #
    # Each one states the situation, why it matters in plain terms, and what
    # the installer can do about it. A finding without a remedy is a complaint;
    # every finding here has one.
    # ═════════════════════════════════════════════════════════════════════

    if (-not $node) {
        $f += [pscustomobject]@{
            Id       = "no-node"
            Severity = "stop"
            Title    = "Node.js is not installed"
            Explain  = "DeepSeek Harness is a Node.js program. Without Node there is nothing to run — this is the harness's own requirement, not the router's."
            Remedy   = "Install Node.js 22.19 or newer (or 24+), then run this installer again."
        }
    } elseif (-not $nodeOk) {
        $f += [pscustomobject]@{
            Id       = "old-node"
            Severity = "stop"
            Title    = "Node.js $node is too old"
            Explain  = "The harness requires version 22.19 or newer, or 24 and above. An older Node may install cleanly and then fail in ways that look like harness bugs."
            Remedy   = "Upgrade Node.js, then run this installer again."
        }
    }

    if ($installs.Count -eq 0) {
        $f += [pscustomobject]@{
            Id       = "no-harness"
            Severity = "stop"
            Title    = "No DeepSeek Harness installation found"
            Explain  = "The router does not ship the harness — it starts copies of the harness you already have. Nothing was found in the usual places, so there is nothing to start."
            Remedy   = "Install the harness first: npm install -g @deepseek-ai/dsh"
        }
    }

    if ($installs.Count -gt 1) {
        $versions = ($installs | ForEach-Object { $_.Version } | Sort-Object -Unique) -join ", "
        $f += [pscustomobject]@{
            Id       = "multiple-harness"
            Severity = "warn"
            Title    = "$($installs.Count) copies of the harness are installed ($versions)"
            Explain  = "This is not automatically a problem, and it is often deliberate: a tool may bundle its own pinned copy so it keeps working even if you upgrade yours. It becomes a problem only when the copy that runs is not the copy you think you are running."
            Remedy   = "The installer will show you all of them and let you choose which one the router uses."
        }
    }

    if ($shimVersion -and $directVersion -and ($shimVersion.Trim() -ne $directVersion.Trim())) {
        $f += [pscustomobject]@{
            Id       = "version-split"
            Severity = "warn"
            Title    = "dsh reports $($shimVersion.Trim()) but the real harness is $($directVersion.Trim())"
            Explain  = "Something on PATH is intercepting the command. Asking 'dsh --version' gets one answer, while actually running a web server uses a different copy. This usually means someone wrote a launcher script to keep two tools from fighting — a sensible fix that leaves a confusing situation behind."
            Remedy   = "The installer can point the router straight at the real harness, so what it reports and what it runs are always the same thing. The launcher is left untouched."
        }
    }

    if ($onPath.Count -gt 1) {
        $first = $onPath[0].Path
        $others = ($onPath | Select-Object -Skip 1 | ForEach-Object { $_.Path }) -join ", "
        $f += [pscustomobject]@{
            Id       = "shadowed"
            Severity = "warn"
            Title    = "More than one dsh on PATH; the first one wins"
            Explain  = "When you type a command, the computer searches PATH from the start and uses the first match. Anything later is invisible to your shell, even though it is installed and working. So '$first' is what runs, and '$others' never will unless something names it directly."
            Remedy   = "The installer names the harness by full path, so it is unaffected by PATH order and cannot be shadowed later."
        }
    }

    if ($running.Count -gt 0) {
        $desc = ($running | ForEach-Object { "pid $($_.Pid) on port $((@($_.Ports) -join ','))" }) -join "; "
        $f += [pscustomobject]@{
            Id       = "running"
            Severity = "info"
            Title    = "A harness is already running ($desc)"
            Explain  = "This is normal and usually desirable — it is the one you are using right now. The router works alongside it and will not touch it. It simply will not reuse those ports."
            Remedy   = "Nothing to do. The router starts numbering above them."
        }
    }

    if ($stateRootExists -and ($workspaces.Count -gt 0 -or $sessionCount -gt 0)) {
        $f += [pscustomobject]@{
            Id       = "existing-state"
            Severity = "info"
            Title    = "Existing harness state found: $($workspaces.Count) workspace(s), $sessionCount session folder(s)"
            Explain  = "You already have projects and conversation history. The router can leave all of it exactly where it is, or make an independent copy for a new instance so that instance can see the same work. It never modifies the original either way."
            Remedy   = "You will be asked which you prefer. There is no wrong answer."
        }
    }

    $inside = @($workspaces | Where-Object { $_.InsideState })
    foreach ($ws in $inside) {
        $f += [pscustomobject]@{
            Id       = "workspace-inside-state"
            Severity = "warn"
            Title    = "The workspace '$($ws.Title)' lives inside the harness state folder"
            Explain  = "Its files are at $($ws.Path), which is inside the folder the harness uses for its own bookkeeping. Two harnesses pointed at one folder is the exact problem the router exists to prevent, so a workspace inside the state folder is too close for comfort."
            Remedy   = "The installer skips it and registers your real project folders instead. You can add it deliberately later if you want it."
        }
    }

    $missing = @($workspaces | Where-Object { -not $_.Exists })
    if ($missing.Count -gt 0) {
        $names = ($missing | ForEach-Object { $_.Title }) -join ", "
        $f += [pscustomobject]@{
            Id       = "workspace-missing"
            Severity = "warn"
            Title    = "$($missing.Count) registered workspace(s) no longer exist on disk: $names"
            Explain  = "The harness remembers a project by its folder path. If the folder is deleted or moved, that entry points at nothing. The harness cannot create it — a path you gave it is not something it will invent."
            Remedy   = "These are skipped. Recreate the folder or point a new instance at wherever the work moved to."
        }
    }

    if ($routerInstalled) {
        $f += [pscustomobject]@{
            Id       = "router-present"
            Severity = "info"
            Title    = "A previous router install was found"
            Explain  = "The router is already set up here. Running the installer again is safe — it will not duplicate anything, and it will not overwrite instances you have added."
            Remedy   = "Nothing to do. The installer is safe to re-run at any time."
        }
    }

    # The ports the router will refuse to take, and why.
    $reserved = @{}
    foreach ($p in 3080, 3081) {
        if (Test-PortListening -Port $p) {
            $owner = Get-ListenerOwner -Port $p
            $reserved[$p] = $owner
        }
    }

    return [pscustomobject]@{
        Node             = [pscustomobject]@{ Version = $node; Ok = $nodeOk }
        PathEntries      = $onPath
        Installs         = $installs
        ShimVersion      = if ($shimVersion) { $shimVersion.Trim() } else { $null }
        ShimPath         = $shimPath
        DirectVersion    = if ($directVersion) { $directVersion.Trim() } else { $null }
        DirectPath       = $directPath
        StateRoot        = $dshHome
        StateRootExists  = $stateRootExists
        Workspaces       = $workspaces
        SessionCount     = $sessionCount
        PortableMb       = $portableMb
        ToolchainMb      = $toolchainMb
        Running          = $running
        RouterHome       = $routerHome
        RouterInstalled  = $routerInstalled
        ReservedPorts    = $reserved
        Findings         = $f
    }
}

function Show-Survey {
    <#
    .SYNOPSIS
    Print a survey in plain language, with an explanation for every finding.
    #>
    param([Parameter(Mandatory)]$Survey)

    Write-Head "What is on this machine"

    Write-Item ("Node.js:            " + $(if ($Survey.Node.Version) { $Survey.Node.Version } else { "not found" })) `
               $(if ($Survey.Node.Ok) { "Gray" } else { "Red" })

    if ($Survey.Installs.Count -gt 0) {
        Write-Item "DeepSeek Harness found:"
        foreach ($i in ($Survey.Installs | Sort-Object { $_.Version } -Descending)) {
            Write-Host ("      {0,-10} {1}" -f $i.Version, $i.Kind) -ForegroundColor Gray
            Write-Host ("                 {0}" -f $i.Path) -ForegroundColor DarkGray
        }
    } else {
        Write-Item "DeepSeek Harness:   not found" "Red"
    }

    if ($Survey.ShimVersion) {
        Write-Item ("'dsh --version' answers:  " + $Survey.ShimVersion) `
            $(if ($Survey.DirectVersion -and $Survey.ShimVersion -ne $Survey.DirectVersion) { "Yellow" } else { "Gray" })
        Write-Host ("                 ({0})" -f $Survey.ShimPath) -ForegroundColor DarkGray
    }

    if ($Survey.StateRootExists) {
        Write-Item ("Existing state:     {0}" -f $Survey.StateRoot)
        Write-Item ("                    {0} workspace(s), {1} session folder(s)" -f $Survey.Workspaces.Count, $Survey.SessionCount)
    } else {
        Write-Item "Existing state:     none (a first-time setup)" "DarkGray"
    }

    if ($Survey.Running.Count -gt 0) {
        $ports = ($Survey.Running | ForEach-Object { @($_.Ports) } | Where-Object { $_ }) -join ", "
        Write-Item ("Already running:    harness on port(s) $ports") "Cyan"
    }

    # Findings, most serious first. Each is stated, explained, and given a
    # remedy — never just a warning.
    if ($Survey.Findings.Count -gt 0) {
        Write-Head "What needs your attention"

        $order = @{ "stop" = 0; "warn" = 1; "info" = 2 }
        foreach ($finding in ($Survey.Findings | Sort-Object { $order[$_.Severity] })) {
            Write-Host ""
            switch ($finding.Severity) {
                "stop" { Write-Bad   $finding.Title }
                "warn" { Write-Warn2 $finding.Title }
                "info" { Write-Info  $finding.Title }
            }
            # The explanation is the deliverable. A user who understands the
            # problem can make their own choice; one who only sees a warning
            # cannot.
            Write-Note $finding.Explain
            Write-Note ("What to do: " + $finding.Remedy)
        }
    }

    Write-Host ""
}
