# DeepSeek Harness Router — the questions
#
# WHY THIS FILE EXISTS
# --------------------
# An installer makes decisions on someone's behalf, and a decision made without
# understanding is one the user cannot revisit or trust. So every question here:
#
#   1. States what is being chosen, in plain words.
#   2. Explains what each option *does*, not just what it is called.
#   3. Says what happens afterwards, including how to undo it.
#   4. Defaults to the safest option, and says why it is the default.
#
# The standard aimed for is "explain it to a smart twelve-year-old": no jargon
# without a definition, no option without a consequence, and never a choice
# where the user has to guess which one is safe.

Set-StrictMode -Version Latest

# ─────────────────────────────────────────────────────────────────────────
# Answering
#
# Two ways in, and the difference matters.
#
#   Interactive  - a person is asked, and can see the explanations.
#   Scripted     - answers are supplied up front, for unattended installs and
#                  for testing. `Read-Host` cannot be driven by piped input, so
#                  a scripted run is the only way to exercise this flow without
#                  a human at the keyboard.
#
# Both paths go through the same functions, so the questions and their validation
# cannot drift apart between the two modes.
# ─────────────────────────────────────────────────────────────────────────

# When set, these are consumed in order instead of prompting.
$script:ScriptedAnswers = $null

function Set-ScriptedAnswers {
    param([string[]]$Answers)
    $script:ScriptedAnswers = [System.Collections.Generic.Queue[string]]::new()
    foreach ($a in $Answers) { $script:ScriptedAnswers.Enqueue($a) }
}

function Test-Scripted {
    return ($null -ne $script:ScriptedAnswers -and $script:ScriptedAnswers.Count -gt 0)
}

function Read-NextScripted {
    if (Test-Scripted) { return $script:ScriptedAnswers.Dequeue() }
    return $null
}

function Write-Option {
    <#
    .SYNOPSIS
    Present one option: a number, a plain name, what it does, and the consequence.
    #>
    param(
        [int]$Number,
        [string]$Name,
        [string]$What,
        [string]$Then,
        [bool]$Recommended = $false
    )
    Write-Host ""
    $label = "      $Number) $Name"
    if ($Recommended) {
        Write-Host $label -ForegroundColor Green -NoNewline
        Write-Host "   (recommended)" -ForegroundColor DarkGreen
    } else {
        Write-Host $label -ForegroundColor White
    }
    foreach ($line in (Format-Wrapped -Text $What -Width 64)) {
        Write-Host "         $line" -ForegroundColor Gray
    }
    foreach ($line in (Format-Wrapped -Text ("Afterwards: " + $Then) -Width 64)) {
        Write-Host "         $line" -ForegroundColor DarkGray
    }
}

function Read-Choice {
    <#
    .SYNOPSIS
    Ask for a number, and keep asking until the answer is valid.

    .DESCRIPTION
    Never guesses. A blank answer takes the default only when one is declared,
    and an invalid answer re-asks with the valid range stated. An installer that
    silently picks something after a typo is worse than one that asks again.
    #>
    param(
        [string]$Prompt,
        [int]$Min,
        [int]$Max,
        [int]$Default = 0
    )

    $hint = if ($Default -ge $Min -and $Default -le $Max) { " [default $Default]" } else { "" }
    while ($true) {
        if (Test-Scripted) {
            $answer = Read-NextScripted
            Write-Host ""
            Write-Host "      Your choice${hint}: $answer" -ForegroundColor DarkGray
        } else {
            Write-Host ""
            # `${hint}` rather than `$hint` — PowerShell reads a bare `$name:` as
            # a scope qualifier (`$script:`, `$env:`), and the colon here is prose.
            Write-Host "      Your choice${hint}: " -ForegroundColor White -NoNewline
            $answer = Read-Host
        }

        if ([string]::IsNullOrWhiteSpace($answer)) {
            if ($Default -ge $Min -and $Default -le $Max) { return $Default }
            Write-Host "      Please type a number between $Min and $Max." -ForegroundColor Yellow
            continue
        }

        $parsed = 0
        if ([int]::TryParse($answer.Trim(), [ref]$parsed) -and $parsed -ge $Min -and $parsed -le $Max) {
            return $parsed
        }
        Write-Host "      '$answer' is not one of the options. Type a number from $Min to $Max." -ForegroundColor Yellow
    }
}

function Read-YesNo {
    param([string]$Prompt, [bool]$Default = $true)
    $suffix = if ($Default) { "[Y/n]" } else { "[y/N]" }
    while ($true) {
        if (Test-Scripted) {
            $answer = Read-NextScripted
            Write-Host ""
            Write-Host "      $Prompt $suffix $answer" -ForegroundColor DarkGray
        } else {
            Write-Host ""
            Write-Host "      $Prompt $suffix " -ForegroundColor White -NoNewline
            $answer = Read-Host
        }
        if ([string]::IsNullOrWhiteSpace($answer)) { return $Default }
        switch ($answer.Trim().ToLower()) {
            "y" { return $true }
            "yes" { return $true }
            "n" { return $false }
            "no" { return $false }
            default { Write-Host "      Please answer y or n." -ForegroundColor Yellow }
        }
    }
}

# ─────────────────────────────────────────────────────────────────────────
# Question 1 — which harness should the router use?
# ─────────────────────────────────────────────────────────────────────────

function Select-Harness {
    <#
    .SYNOPSIS
    Choose which installed copy of the harness the router will start.

    .DESCRIPTION
    Several copies can exist, and the one that runs is not always the one that
    answers `--version`. This asks which to use and explains the difference,
    because choosing wrong is invisible until something breaks.

    Returns the chosen install object, or $null to abort.
    #>
    param([Parameter(Mandatory)]$Survey)

    $installs = @($Survey.Installs | Sort-Object { $_.Version } -Descending)
    if ($installs.Count -eq 0) { return $null }
    if ($installs.Count -eq 1) {
        Write-Host ""
        Write-Host "      Only one copy of the harness is installed, so there is nothing to" -ForegroundColor Gray
        Write-Host "      choose. Using it:" -ForegroundColor Gray
        Write-Host "        $($installs[0].Version)  —  $($installs[0].Path)" -ForegroundColor White
        return $installs[0]
    }

    Write-Head "Which harness should the router start?"

    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "Your computer has more than one copy of DeepSeek Harness. This happens when a tool installs its own private copy so it cannot be broken by an upgrade. It is not a mistake — but only one copy can be the one the router runs." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }
    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "The number in each option is that copy's version. A higher number is newer." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }

    $n = 0
    $map = @{}
    foreach ($i in $installs) {
        $n++
        $map[$n] = $i

        # Explain each copy in terms of what it is FOR, not where it lives.
        $what = switch ($i.Kind) {
            "npm global" {
                "The one your shell would normally run. Installed with npm, and the copy that stays up to date when you upgrade the harness yourself."
            }
            "composed profile" {
                "The harness's own working copy, assembled from many small packages. It lives inside your harness data folder and is what a running harness actually executes."
            }
            "bundled toolchain" {
                "A private copy shipped by another program, pinned to an exact version so that program keeps working. Upgrading it is that program's business, not yours."
            }
            default { "A copy of the harness." }
        }
        $then = switch ($i.Kind) {
            "npm global"          { "New instances run this version. Upgrading with npm changes what they run." }
            "composed profile"    { "New instances run the version already assembled here." }
            "bundled toolchain"   { "New instances run this older, pinned version. Another program's upgrades would change it." }
            default               { "New instances run this copy." }
        }

        Write-Option -Number $n -Name "$($i.Version)  ($($i.Kind))" -What $what -Then $then `
            -Recommended ($i.Kind -eq "npm global")
    }

    Write-Host ""
    Write-Host "      $(($n + 1))) Cancel the install" -ForegroundColor DarkGray

    $choice = Read-Choice -Prompt "Harness" -Min 1 -Max ($n + 1) -Default 1
    if ($choice -eq ($n + 1)) { return $null }
    return $map[$choice]
}

# ─────────────────────────────────────────────────────────────────────────
# Question 2 — what should happen to existing projects and history?
# ─────────────────────────────────────────────────────────────────────────

function Select-StateStrategy {
    <#
    .SYNOPSIS
    Decide what a new instance starts with: nothing, or a copy of existing work.

    .DESCRIPTION
    Every instance gets its own private folder for its bookkeeping. That privacy
    is the whole point — it is what stops two harnesses from overwriting each
    other's records. But it also means a new instance starts blank unless we
    deliberately copy something in.

    Returns a hashtable describing the choice.
    #>
    param([Parameter(Mandatory)]$Survey)

    if (-not $Survey.StateRootExists -or ($Survey.Workspaces.Count -eq 0 -and $Survey.SessionCount -eq 0)) {
        return @{ Strategy = "empty" }
    }

    Write-Head "What should the new instance start with?"

    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "You already have work saved: $($Survey.Workspaces.Count) project(s) and $($Survey.SessionCount) folder(s) of conversation history." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }
    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "Each instance the router starts keeps its own records, so that two instances can never overwrite each other. The question is what to put in this one's records to begin with." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }

    $size = $Survey.PortableMb

    Write-Option -Number 1 -Name "Copy my projects and history into the new instance" `
        -What "The new instance gets its own copy of your project list, conversation history and settings, so its window looks like the one you use today. Your original files are not touched or moved — they stay exactly where they are, and remain your backup." `
        -Then "About ${size} MB is copied. A fresh instance shows the same projects and history as your current one. Changes you make in the copy do not appear in the original, and vice versa — they are separate from that point on." `
        -Recommended $true

    Write-Option -Number 2 -Name "Start empty — new instances are for new work" `
        -What "The new instance begins with a blank slate. Your existing projects and history stay available in the harness you already use; the router's instances are separate and clean." `
        -Then "Nothing is copied, so it is instant. A fresh instance shows no projects until you add some. Best if you want a clear separation between 'my existing work' and 'the new parallel instances'."

    Write-Option -Number 3 -Name "Copy projects but not conversation history" `
        -What "The project list comes across so the new instance knows where your work lives, but the conversation history stays behind." `
        -Then "A small copy. The instance can open your projects but the chat history appears empty. Useful if history is large or private."

    $choice = Read-Choice -Prompt "Starting point" -Min 1 -Max 3 -Default 1
    switch ($choice) {
        1 { return @{ Strategy = "full";      CopyHistory = $true;  CopySettings = $true;  RegisterWorkspaces = $true } }
        2 { return @{ Strategy = "empty";     CopyHistory = $false; CopySettings = $false; RegisterWorkspaces = $false } }
        3 { return @{ Strategy = "projects";  CopyHistory = $false; CopySettings = $true;  RegisterWorkspaces = $true } }
    }
}

# ─────────────────────────────────────────────────────────────────────────
# Question 3 — which folders should become projects?
# ─────────────────────────────────────────────────────────────────────────

function Select-Workspaces {
    <#
    .SYNOPSIS
    Choose which of the existing project folders the new instance should know about.

    .DESCRIPTION
    Two kinds of entry are excluded by default, each for a specific reason that
    is explained rather than assumed. The user can override either — this asks,
    it does not forbid.
    #>
    param([Parameter(Mandatory)]$Survey)

    $eligible = @($Survey.Workspaces | Where-Object { -not $_.InsideState -and $_.Exists })
    $inside   = @($Survey.Workspaces | Where-Object { $_.InsideState })
    $missing  = @($Survey.Workspaces | Where-Object { -not $_.Exists -and -not $_.InsideState })

    if ($eligible.Count -eq 0) {
        return @{ Chosen = @(); Skipped = @($Survey.Workspaces); SkippedInside = $inside; SkippedMissing = $missing }
    }

    Write-Head "Which project folders should the new instance know about?"

    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "The harness remembers your projects by their folder on disk. A project is just a folder it opens — nothing is copied or moved, and the router never writes into it." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }

    Write-Host ""
    Write-Host "      Found:" -ForegroundColor White
    foreach ($w in $eligible) {
        Write-Host ("        {0,-22} {1}" -f $w.Title, $w.Path) -ForegroundColor Gray
        Write-Host ("        {0,-22} {1} conversation(s)" -f "", $w.SessionCount) -ForegroundColor DarkGray
    }

    if ($inside.Count -gt 0) {
        Write-Host ""
        Write-Host "      Left out on purpose:" -ForegroundColor Yellow
        foreach ($w in $inside) {
            Write-Host ("        {0,-22} {1}" -f $w.Title, $w.Path) -ForegroundColor Gray
            foreach ($line in (Format-Wrapped -Text "This folder is inside the harness's own data folder. Sharing it between two harnesses could corrupt both, because each would write its records into the other's territory. It stays with your original harness." -Width 62)) {
                Write-Host "        $line" -ForegroundColor DarkGray
            }
        }
    }

    if ($missing.Count -gt 0) {
        Write-Host ""
        Write-Host "      No longer on disk:" -ForegroundColor Yellow
        foreach ($w in $missing) {
            Write-Host ("        {0,-22} {1}" -f $w.Title, $w.Path) -ForegroundColor Gray
        }
        foreach ($line in (Format-Wrapped -Text "The folder was deleted or moved, so this entry points at nothing. Recreate the folder to use it again." -Width 62)) {
            Write-Host "        $line" -ForegroundColor DarkGray
        }
    }

    Write-Host ""
    if (Read-YesNo -Prompt "Add these $($eligible.Count) project folder(s) to the new instance?" -Default $true) {
        return @{ Chosen = $eligible; Skipped = @(); SkippedInside = $inside; SkippedMissing = $missing }
    }
    return @{ Chosen = @(); Skipped = $eligible; SkippedInside = $inside; SkippedMissing = $missing }
}

# ─────────────────────────────────────────────────────────────────────────
# Question 4 — API keys
# ─────────────────────────────────────────────────────────────────────────

function Select-Credentials {
    <#
    .SYNOPSIS
    Decide how a new instance gets permission to talk to the AI.

    .DESCRIPTION
    Without a key an instance starts, looks healthy, and fails every request.
    That is a confusing failure, so the choice is made explicit here rather than
    discovered later.
    #>
    param([Parameter(Mandatory)]$Survey)

    $credPath = Join-Path $Survey.StateRoot ".credentials.yaml"
    $hasKeys = Test-Path $credPath

    Write-Head "How should the new instance get permission to use the AI?"

    Write-Host ""
    foreach ($line in (Format-Wrapped -Text "Talking to an AI model needs an API key — a password that proves the request is yours. The harness keeps keys in a small file in its data folder." -Width 68)) {
        Write-Host "      $line" -ForegroundColor Gray
    }

    if (-not $hasKeys) {
        Write-Option -Number 1 -Name "I will add my keys later in the browser interface" `
            -What "The instance starts with no keys. It will run and show its interface normally, but any request to a model will fail until you add a key." `
            -Then "You can add keys in the harness settings, or re-run this installer later." `
            -Recommended $true
        $null = Read-Choice -Prompt "Keys" -Min 1 -Max 1 -Default 1
        return @{ Mode = "none" }
    }

    Write-Option -Number 1 -Name "Share the keys I already have" `
        -What "A link is created rather than a copy, so the new instance and your existing harness point at the same key file. The keys are never duplicated, so rotating a key updates both at once." `
        -Then "New instances can use models immediately. Turning this off later with 'router edit --no-share-credentials' replaces the link with a private empty file." `
        -Recommended $true

    Write-Option -Number 2 -Name "Give the new instance its own empty key file" `
        -What "The instance gets a separate, empty key file. It cannot see or affect your existing keys at all." `
        -Then "Every model request fails until you add keys for this instance. Choose this if you want a hard wall between the two."

    Write-Option -Number 3 -Name "Do not set up keys now" `
        -What "Nothing is configured. The instance starts and you decide later." `
        -Then "You can add keys in the browser interface, or re-run this installer."

    $choice = Read-Choice -Prompt "Keys" -Min 1 -Max 3 -Default 1
    switch ($choice) {
        1 { return @{ Mode = "share" } }
        2 { return @{ Mode = "own" } }
        3 { return @{ Mode = "none" } }
    }
}
