# Run from dev.cmd. Each watcher owns one terminal window and its private copies of the binaries.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$buildDir = Join-Path $root 'target\watch\build'
$runDir = Join-Path $root ('target\watch\sessions\' + [guid]::NewGuid().ToString('N'))
$terminal = $null
$generation = 0

function Write-Status([string]$Message) {
    Write-Host ('[{0:HH:mm:ss}] {1}' -f [datetime]::Now, $Message)
}

function Get-SourceStamp {
    $files = @(
        foreach ($name in 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo\config', '.cargo\config.toml') {
            Get-Item -LiteralPath (Join-Path $root $name) -ErrorAction SilentlyContinue
        }
        foreach ($crate in Get-ChildItem -LiteralPath (Join-Path $root 'crates') -Directory) {
            foreach ($name in 'Cargo.toml', 'build.rs', 'winshrc') {
                Get-Item -LiteralPath (Join-Path $crate.FullName $name) -ErrorAction SilentlyContinue
            }
            foreach ($directory in 'src', 'assets') {
                Get-ChildItem -LiteralPath (Join-Path $crate.FullName $directory) -File -Recurse -ErrorAction SilentlyContinue
            }
        }
    )
    ($files | Sort-Object FullName | ForEach-Object {
        '{0}|{1}|{2}' -f $_.FullName, $_.Length, $_.LastWriteTimeUtc.Ticks
    }) -join [environment]::NewLine
}

function Stop-DevTerminal([Diagnostics.Process]$Process) {
    if ($null -eq $Process) { return }
    try {
        if (!$Process.HasExited -and (!$Process.CloseMainWindow() -or !$Process.WaitForExit(3000))) {
            $Process.Kill()
            $Process.WaitForExit()
        }
    } catch [InvalidOperationException] {
        if (!$Process.HasExited) { throw }
    } finally {
        $Process.Dispose()
    }
}

function Remove-Generation([int]$Number) {
    $dir = Join-Path $runDir $Number
    if (Test-Path -LiteralPath $dir) {
        foreach ($name in 'winsh.exe', 'winsh-terminal.exe') {
            $file = Join-Path $dir $name
            if (Test-Path -LiteralPath $file) { [IO.File]::Delete($file) }
        }
        [IO.Directory]::Delete($dir)
    }
}

Push-Location -LiteralPath $root
try {
    $null = Get-Command cargo -CommandType Application
    $null = [IO.Directory]::CreateDirectory($runDir)
    Write-Status "Watching $root. A successful rebuild restarts the dev terminal and its shell session."
    $attempted = $null
    $runningHash = $null
    while ($null -eq $terminal -or !$terminal.HasExited) {
        $stamp = Get-SourceStamp
        if ($stamp -ceq $attempted) {
            Start-Sleep -Milliseconds 400
            continue
        }
        # Editors may save several files or replace one via a temporary file.
        Start-Sleep -Milliseconds 300
        if ((Get-SourceStamp) -cne $stamp) { continue }
        $attempted = $stamp
        Write-Status 'Building winsh and winsh-terminal...'
        $artifacts = @{}
        & cargo build --workspace --bins --target-dir $buildDir --message-format=json-render-diagnostics | ForEach-Object {
            $event = ConvertFrom-Json -InputObject $_
            if ($event.reason -eq 'compiler-artifact' -and $event.executable) {
                $artifacts[$event.target.name] = $event.executable
            }
        }
        if ($LASTEXITCODE -ne 0) {
            Write-Status 'Build failed. Waiting for another edit.'
            continue
        }
        if ($terminal -and $terminal.HasExited) { break }
        if ((Get-SourceStamp) -cne $stamp) {
            Write-Status 'Files changed during the build. Building the latest edits.'
            continue
        }

        $hash = (@('winsh', 'winsh-terminal') | ForEach-Object {
            (Get-FileHash -LiteralPath $artifacts[$_] -Algorithm SHA256).Hash
        }) -join '/'
        if ($hash -eq $runningHash) {
            Write-Status 'Build succeeded. The running binaries are up to date.'
            continue
        }

        $next = $generation + 1
        $dir = Join-Path $runDir $next
        $null = [IO.Directory]::CreateDirectory($dir)
        try {
            foreach ($name in 'winsh', 'winsh-terminal') {
                Copy-Item -LiteralPath $artifacts[$name] -Destination (Join-Path $dir "$name.exe")
            }
            $label = "#{0} at {1:yyyy-MM-dd HH:mm:ss}" -f $next, [datetime]::Now
            $start = New-Object Diagnostics.ProcessStartInfo
            $start.FileName = Join-Path $dir 'winsh-terminal.exe'
            $start.WorkingDirectory = $root
            $start.UseShellExecute = $false
            $start.CreateNoWindow = $true
            $start.EnvironmentVariables['WINSH_DEV_BUILD'] = $label
            $replacement = [Diagnostics.Process]::Start($start)
        } catch {
            Remove-Generation $next
            Write-Warning "Could not start the new build. $($_.Exception.Message)"
            Write-Status 'Waiting for another edit.'
            continue
        }
        $previous = $terminal
        $terminal = $replacement
        $generation = $next
        $runningHash = $hash
        Write-Status "Started dev build $label (terminal PID $($terminal.Id))."
        Stop-DevTerminal $previous
        Remove-Generation ($generation - 1)
    }
} finally {
    try {
        Stop-DevTerminal $terminal
        Remove-Generation $generation
        if (Test-Path -LiteralPath $runDir) { [IO.Directory]::Delete($runDir) }
    } finally {
        Pop-Location
    }
}
