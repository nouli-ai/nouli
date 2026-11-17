# Install a published Windows Nouli binary. Set NOULI_VERSION or NOULI_INSTALL_DIR to override defaults.
# Runs in its own scope so piping into Invoke-Expression does not change the caller's preferences.
& {
    $ErrorActionPreference = 'Stop'
    function Remove-NouliTemporary {
        param([string]$Path, [bool]$Tree = $false)
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            try {
                if ($Tree) {
                    if ([IO.Directory]::Exists($Path)) { [IO.Directory]::Delete($Path, $true) }
                } else {
                    [IO.File]::Delete($Path)
                }
                return
            } catch {
                $reason = $_.Exception.GetBaseException()
                if ($attempt -eq 29 -or ($reason -isnot [IO.IOException] -and $reason -isnot [UnauthorizedAccessException])) { throw }
                # Windows can briefly retain an executable image after its process exits.
                Start-Sleep -Milliseconds 100
            }
        }
    }
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT -or [Environment]::OSVersion.Version.Build -lt 22000) {
        throw 'Nouli requires Windows 11 or newer.'
    }
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    # Older .NET versions can report the emulated process architecture. Windows supplies the native value here.
    if ($env:PROCESSOR_ARCHITEW6432) { $architecture = $env:PROCESSOR_ARCHITEW6432 }
    $target = switch ($architecture.ToLowerInvariant()) {
        { $_ -in 'arm64', 'aarch64' } { 'aarch64-pc-windows-msvc'; break }
        { $_ -in 'x64', 'amd64' } { 'x86_64-pc-windows-msvc'; break }
        default { throw "Unsupported Windows architecture: $architecture" }
    }
    $version = $env:NOULI_VERSION
    if (!$version -or $version -eq 'latest') {
        $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/nouli-ai/nouli/releases/latest' -Headers @{ Accept = 'application/vnd.github+json' } -TimeoutSec 30
        $version = [string]$release.tag_name
    }
    $version = $version -replace '^v', ''
    if ($version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw 'NOULI_VERSION must be latest or a stable version such as 0.6.0.'
    }
    $installDir = $env:NOULI_INSTALL_DIR
    if (!$installDir) {
        if (!$env:LOCALAPPDATA) { throw 'Set LOCALAPPDATA or NOULI_INSTALL_DIR to choose an installation directory.' }
        $installDir = Join-Path $env:LOCALAPPDATA 'Programs\Nouli\bin'
    }
    $installDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($installDir)
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('nouli-install-' + [Guid]::NewGuid().ToString('N'))
    $staged = $null
    [IO.Directory]::CreateDirectory($temporary) | Out-Null
    try {
        $archiveName = "nouli-$version-$target.zip"
        $archivePath = Join-Path $temporary $archiveName
        $checksumPath = "$archivePath.sha256"
        $base = "https://github.com/nouli-ai/nouli/releases/download/v$version"
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$archiveName" -OutFile $archivePath -TimeoutSec 300
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$archiveName.sha256" -OutFile $checksumPath -TimeoutSec 30
        $checksum = [IO.File]::ReadAllText($checksumPath).Trim()
        if ($checksum -cnotmatch ('^([a-fA-F0-9]{64})  ' + [regex]::Escape($archiveName) + '$')) {
            throw 'Invalid release checksum.'
        }
        $expected = $Matches[1]
        if ((Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -ine $expected) {
            throw 'Release checksum mismatch; existing installation was not changed.'
        }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [IO.Compression.ZipFile]::OpenRead($archivePath)
        try {
            $entries = @($archive.Entries | Where-Object { $_.FullName -ceq 'nouli.exe' })
            if ($entries.Count -ne 1 -or $entries[0].Length -gt 268435456) {
                throw 'Archive must contain one nouli.exe no larger than 256 MiB.'
            }
            # Extract only the root executable; other entries cannot write paths outside the temporary directory.
            $extracted = Join-Path $temporary 'nouli.exe'
            $source = $entries[0].Open()
            try {
                $output = [IO.File]::Open($extracted, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
                try {
                    $buffer = New-Object byte[] 65536
                    $total = [long]0
                    while (($count = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        if ($count -gt 268435456 - $total) { throw 'Executable exceeds the 256 MiB extraction limit.' }
                        $output.Write($buffer, 0, $count)
                        $total += $count
                    }
                    if ($total -ne $entries[0].Length) { throw 'Executable length does not match the archive.' }
                } finally {
                    $output.Dispose()
                }
            } finally {
                $source.Dispose()
            }
        } finally {
            $archive.Dispose()
        }
        # Own and dispose the verification process before publishing or deleting its image.
        $check = New-Object Diagnostics.Process
        $check.StartInfo = New-Object Diagnostics.ProcessStartInfo -Property @{
            FileName = $extracted
            Arguments = '--version'
            UseShellExecute = $false
            CreateNoWindow = $true
            RedirectStandardOutput = $true
            RedirectStandardError = $true
        }
        $started = $false
        try {
            $started = $check.Start()
            if (!$started) { throw 'Cannot start the downloaded executable.' }
            $stdout = $check.StandardOutput.ReadToEndAsync()
            $stderr = $check.StandardError.ReadToEndAsync()
            if (!$check.WaitForExit(10000) -or !$stdout.Wait(1000) -or !$stderr.Wait(1000)) {
                throw 'Downloaded executable version check timed out.'
            }
            $reported = $stdout.Result.TrimEnd([char[]]"`r`n")
            if ($check.ExitCode -ne 0 -or $reported -cne "nouli $version") {
                throw 'The downloaded executable cannot run or its version does not match the release; existing installation was not changed.'
            }
        } finally {
            try {
                if ($started -and !$check.HasExited) { $check.Kill(); [void]$check.WaitForExit(1000) }
            } finally {
                $check.Dispose()
            }
        }
        [IO.Directory]::CreateDirectory($installDir) | Out-Null
        $destination = Join-Path $installDir 'nouli.exe'
        if ([IO.Directory]::Exists($destination)) { throw "$destination is a directory." }
        $staged = Join-Path $installDir ('.nouli-' + [Guid]::NewGuid().ToString('N') + '.exe')
        [IO.File]::Copy($extracted, $staged)
        # Finish cleanup before publication, so cleanup failure also preserves an existing install.
        Remove-NouliTemporary $temporary $true
        $temporary = $null
        try {
            if ([IO.File]::Exists($destination)) {
                [IO.File]::Replace($staged, $destination, [NullString]::Value)
            } else {
                [IO.File]::Move($staged, $destination)
            }
        } catch {
            throw "Cannot replace $destination. Close Nouli and check directory permissions, then retry. The previous executable was not removed. $($_.Exception.Message)"
        }
        $staged = $null
        Write-Output "Installed Nouli $version to $destination"
        if (($env:PATH -split ';').TrimEnd('\') -inotcontains $installDir.TrimEnd('\')) {
            Write-Output "Add $installDir to your user PATH to run nouli from any terminal."
            Write-Output "Run it now with: & '$($destination.Replace("'", "''"))'"
        }
    } finally {
        try {
            if ($staged) { Remove-NouliTemporary $staged }
        } finally {
            if ($temporary) { Remove-NouliTemporary $temporary $true }
        }
    }
}
