# SPDX-License-Identifier: AGPL-3.0-only
# Only native wsl.exe --list --quiet/--verbose; raw rows stay in memory, never artifacts/logs.
function ConvertFrom-FerriprintWslListBytes([IO.MemoryStream]$Stream) {
    $length = [int]$Stream.Length
    if ($length -eq 0) { return [pscustomobject]@{ text=''; encoding='empty' } }
    $bytes = $Stream.GetBuffer()
    $offset=0; $encodingName='utf16-le-no-bom'
    $encoding = [Text.UnicodeEncoding]::new($false,$false,$true)
    if ($length -ge 2 -and $bytes[0] -eq 255 -and $bytes[1] -eq 254) {
        $offset=2; $encodingName='utf16-le-bom'
    } elseif ($length -ge 2 -and $bytes[0] -eq 254 -and $bytes[1] -eq 255) {
        $offset=2; $encodingName='utf16-be-bom'; $encoding=[Text.UnicodeEncoding]::new($true,$false,$true)
    } elseif ($length -ge 3 -and $bytes[0] -eq 239 -and $bytes[1] -eq 187 -and $bytes[2] -eq 191) {
        $offset=3; $encodingName='utf8-bom'; $encoding=[Text.UTF8Encoding]::new($false,$true)
    }
    if ($encodingName.StartsWith('utf16') -and (($length-$offset) % 2 -ne 0)) { throw 'Incomplete native WSL Unicode output' }
    $text=$encoding.GetString($bytes,$offset,$length-$offset)
    if ($text -match '[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]') { throw 'Unexpected controls in decoded native WSL metadata' }
    if ($text.Length -gt 0 -and $text.IndexOf("`r") -lt 0 -and $text.IndexOf("`n") -lt 0) { throw 'Native WSL metadata is not a complete Unicode line response' }
    [pscustomobject]@{ text=$text; encoding=$encodingName }
}

function Invoke-FerriprintWslList([ValidateSet('quiet','verbose')][string]$Mode) {
    $start=[Diagnostics.ProcessStartInfo]::new()
    $start.FileName=Join-Path $env:SystemRoot 'System32\wsl.exe'
    $start.UseShellExecute=$false; $start.CreateNoWindow=$true
    $start.RedirectStandardInput=$true; $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true
    $start.ArgumentList.Add('--list'); $start.ArgumentList.Add('--all'); $start.ArgumentList.Add('--'+$Mode)
    $process=[Diagnostics.Process]::new(); $process.StartInfo=$start
    $stdout=[IO.MemoryStream]::new(); $stderr=[IO.MemoryStream]::new()
    $exitCode=$null; $timedOut=$false
    try {
        if (-not $process.Start()) { throw 'Native WSL metadata process did not start' }
        $process.StandardInput.Close()
        $outCopy=$process.StandardOutput.BaseStream.CopyToAsync($stdout)
        $errCopy=$process.StandardError.BaseStream.CopyToAsync($stderr)
        if (-not $process.WaitForExit(20000)) {
            $timedOut=$true
            $process.Kill($true)
            if (-not $process.WaitForExit(5000)) { throw 'Owned metadata process exit deadline exceeded' }
        }
        $exitCode=$process.ExitCode
        $drain=[Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]]@($outCopy,$errCopy))
        if (-not $drain.Wait(5000)) { $timedOut=$true; throw 'Owned metadata pipe deadline exceeded' }
        $outCopy.GetAwaiter().GetResult(); $errCopy.GetAwaiter().GetResult()
        $decodedOut=ConvertFrom-FerriprintWslListBytes $stdout
        $decodedErr=ConvertFrom-FerriprintWslListBytes $stderr
        [pscustomobject]@{
            exit_code=$exitCode; timed_out=$timedOut; decoded=$true; error_type=$null
            stdout_bytes=$stdout.Length; stderr_bytes=$stderr.Length
            stdout_encoding=$decodedOut.encoding; stderr_encoding=$decodedErr.encoding
            stdout_text=$decodedOut.text; stderr_empty=($decodedErr.text.Trim().Length -eq 0)
        }
    } catch {
        [pscustomobject]@{
            exit_code=$exitCode; timed_out=$timedOut; decoded=$false; error_type=$_.Exception.GetType().FullName
            stdout_bytes=$stdout.Length; stderr_bytes=$stderr.Length
            stdout_encoding=$null; stderr_encoding=$null; stdout_text=''; stderr_empty=$false
        }
    } finally {
        $process.Dispose(); $stdout.Dispose(); $stderr.Dispose()
    }
}

function Get-FerriprintWslState([string]$DistributionName) {
    if ($DistributionName -cnotmatch '^ferriprint-local-qa-[0-9]+$') { throw 'Not the exact owned ASCII run namespace' }
    $quiet=Invoke-FerriprintWslList -Mode quiet
    $record=[ordered]@{
        query='native wsl.exe --list --all --quiet; verbose only for an owned present name'
        quiet_exit=$quiet.exit_code; quiet_timed_out=$quiet.timed_out; quiet_decoded=$quiet.decoded
        quiet_error_type=$quiet.error_type; quiet_stdout_bytes=$quiet.stdout_bytes; quiet_stderr_bytes=$quiet.stderr_bytes
        quiet_encoding=$quiet.stdout_encoding; quiet_stderr_empty=$quiet.stderr_empty
        quiet_owned_rows=@(); quiet_total_row_count=$null
        verbose_not_queried=$true; verbose_exit=$null; verbose_timed_out=$null; verbose_decoded=$null
        verbose_error_type=$null; verbose_stdout_bytes=$null; verbose_stderr_bytes=$null
        verbose_encoding=$null; verbose_stderr_empty=$null; verbose_owned_rows=@()
        registration_observed=$false; registered=$null; version_observed=$false; version=$null
    }
    if ($quiet.exit_code -ne 0 -or $quiet.timed_out -or -not $quiet.decoded -or -not $quiet.stderr_empty) { return [pscustomobject]$record }
    $quietRows=@($quiet.stdout_text -split '\r\n|\r|\n' | ForEach-Object { $_.Trim() } | Where-Object { $_.Length -gt 0 })
    $quietOwned=@($quietRows | Where-Object { [string]::Equals($_,$DistributionName,[StringComparison]::Ordinal) })
    $record.quiet_owned_rows=$quietOwned
    $record.quiet_total_row_count=$quietRows.Count
    # The documented --all --quiet success path prints names only; zero rows is a valid empty set.
    if ($quietOwned.Count -eq 0) {
        $record.registration_observed=$true; $record.registered=$false
        return [pscustomobject]$record
    }
    if ($quietOwned.Count -ne 1) { return [pscustomobject]$record }
    $record.verbose_not_queried=$false
    $verbose=Invoke-FerriprintWslList -Mode verbose
    $record.verbose_exit=$verbose.exit_code; $record.verbose_timed_out=$verbose.timed_out
    $record.verbose_decoded=$verbose.decoded; $record.verbose_error_type=$verbose.error_type
    $record.verbose_stdout_bytes=$verbose.stdout_bytes; $record.verbose_stderr_bytes=$verbose.stderr_bytes
    $record.verbose_encoding=$verbose.stdout_encoding; $record.verbose_stderr_empty=$verbose.stderr_empty
    if ($verbose.exit_code -ne 0 -or $verbose.timed_out -or -not $verbose.decoded -or -not $verbose.stderr_empty) { return [pscustomobject]$record }
    $verboseRows=@($verbose.stdout_text -split '\r\n|\r|\n' | Where-Object { $_.Trim().Length -gt 0 })
    $prefix='^\s*(?:\*\s*)?'+[regex]::Escape($DistributionName)+'(?=\s|$)'
    $verboseOwned=@($verboseRows | Where-Object { [regex]::IsMatch($_,$prefix) })
    foreach ($row in $verboseOwned) {
        $match=[regex]::Match($row,'^\s*(?:\*\s*)?'+[regex]::Escape($DistributionName)+'\s+.+?\s+([0-9]+)\s*$')
        [int]$number=0
        $parsed=$match.Success -and [int]::TryParse($match.Groups[1].Value,[ref]$number)
        $record.verbose_owned_rows += [pscustomobject]@{ name=$DistributionName; row_format_valid=$parsed; version=$(if ($parsed) { $number } else { $null }) }
    }
    if ($verboseOwned.Count -eq 1 -and $record.verbose_owned_rows[0].row_format_valid) {
        $record.registration_observed=$true; $record.registered=$true
        $record.version_observed=$true; $record.version=$record.verbose_owned_rows[0].version
    }
    [pscustomobject]$record
}
