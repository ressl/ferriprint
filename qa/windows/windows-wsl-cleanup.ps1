# SPDX-License-Identifier: AGPL-3.0-only
# Always-step recovery for this one disposable hosted VM; never cancel a job.
param([Parameter(Mandatory=$true)][string]$Work)
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_RUN_ID -notmatch '^\d+$') { throw 'Missing owned run namespace' }
$expectedWork = Join-Path $env:RUNNER_TEMP ('ferriprint-windows-local-qa-' + $env:GITHUB_RUN_ID)
if (-not [IO.Path]::GetFullPath($Work).Equals([IO.Path]::GetFullPath($expectedWork), [StringComparison]::OrdinalIgnoreCase)) { throw 'Work directory is not the exact owned run directory' }
$proof = Join-Path $Work 'proof'
if (-not (Test-Path -LiteralPath $proof)) { return }
$ownershipPath = Join-Path $proof 'ownership.json'
$receipt = [ordered]@{ scope='Only recorded absent-before owned resources in this disposable hosted VM'; no_job_cancellation_api=$true }
if (-not (Test-Path -LiteralPath $ownershipPath)) {
  $receipt.no_resources_claimed_by_main = $true
  $receipt | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $proof 'always-cleanup-receipt.json') -Encoding UTF8
  return
}
$owned = Get-Content -LiteralPath $ownershipPath -Raw | ConvertFrom-Json
$queue = 'Ferriprint LOCAL NONPRINTING QA ' + $env:GITHUB_RUN_ID
$distro = 'ferriprint-local-qa-' + $env:GITHUB_RUN_ID
if ($owned.queue_name -cne $queue -or $owned.distro_name -cne $distro) { throw 'Owned resource receipt namespace mismatch' }
$failures = [Collections.Generic.List[string]]::new()
function Assert-OwnedTeardown {
  if ($owned.runtime_start_attempted -and (-not $owned.zero_job_teardown_proven -or $owned.violation_observed)) {
    $receipt.runtime_preserved_on_disposable_vm=$true
    $receipt.preservation_scope='No queue/state/distro teardown or job/credential export; provider automatic VM disposal is outside this controller'
    $receipt.acceptance=$false
    $receipt.failures=@($failures)
    $receipt | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $proof 'always-cleanup-receipt.json') -Encoding UTF8
    throw 'Runtime started without a clean actual final zero-job proof, or a violation was observed; fixture history preserved on disposable VM'
  }
}
Assert-OwnedTeardown
. (Join-Path $Work 'windows-wsl-metadata.ps1')
try {
  if ($owned.queue_absent_before) {
    $current = Get-Printer | Where-Object Name -eq $queue
    if ($null -ne $current) {
      $jobs = @(Get-PrintJob -PrinterName $queue)
      $receipt.owned_native_job_count_before_removal = $jobs.Count
      if ($jobs.Count -eq 0) { Remove-Printer -Name $queue }
      else { throw 'Unexpected native jobs preserved; no cancel or implicit queue removal' }
    }
    $receipt.owned_native_queue_absent = ($null -eq (Get-Printer -Name $queue -ErrorAction SilentlyContinue))
    if (-not $receipt.owned_native_queue_absent) { $failures.Add('Owned native queue remains') }
    if ($owned.owned_port_absent_before -and $owned.owned_port_name) {
      if (-not (Get-Printer | Where-Object PortName -eq $owned.owned_port_name)) {
        if (Get-PrinterPort -Name $owned.owned_port_name -ErrorAction SilentlyContinue) { Remove-PrinterPort -Name $owned.owned_port_name }
      }
      $receipt.recorded_owned_port_absent = ($null -eq (Get-PrinterPort -Name $owned.owned_port_name -ErrorAction SilentlyContinue))
      if (-not $receipt.recorded_owned_port_absent) { $failures.Add('Recorded owned port remains or is in use') }
    }
    if ($owned.inbox_ipp_registered_before -eq $false) {
      if (-not (Get-Printer | Where-Object DriverName -eq 'Microsoft IPP Class Driver')) {
        if (Get-PrinterDriver -Name 'Microsoft IPP Class Driver' -ErrorAction SilentlyContinue) { Remove-PrinterDriver -Name 'Microsoft IPP Class Driver' }
      }
      $receipt.new_inbox_registration_removed = ($null -eq (Get-PrinterDriver -Name 'Microsoft IPP Class Driver' -ErrorAction SilentlyContinue))
      if (-not $receipt.new_inbox_registration_removed) { $failures.Add('New inbox registration remains or is in use; no package/driver-store removal is allowed') }
    }
  }
} catch {
  $failures.Add('Native cleanup error type: ' + $_.Exception.GetType().FullName)
  if ($owned.runtime_start_attempted) {
    $owned.zero_job_teardown_proven=$false; $owned.violation_observed=$true
    $owned | ConvertTo-Json | Set-Content -LiteralPath $ownershipPath -Encoding UTF8
  }
}
Assert-OwnedTeardown
try {
  if ($owned.distro_absent_before) {
    $receipt.owned_wsl_unregister_called=$false
    $receipt.owned_wsl_unregister_exit=$null
    $receipt.owned_wsl_before_teardown = Get-FerriprintWslState -DistributionName $distro
    if (-not $receipt.owned_wsl_before_teardown.registration_observed) { throw 'Owned WSL CLI registration unknown; no teardown or absence claim' }
    if ($receipt.owned_wsl_before_teardown.registered) {
      & wsl.exe --terminate $distro
      $receipt.owned_wsl_unregister_called=$true
      & wsl.exe --unregister $distro
      $receipt.owned_wsl_unregister_exit=$LASTEXITCODE
    }
    $receipt.owned_wsl_after_teardown = Get-FerriprintWslState -DistributionName $distro
    $receipt.owned_wsl_registration_absent = $receipt.owned_wsl_after_teardown.registration_observed -and -not $receipt.owned_wsl_after_teardown.registered
    if (($receipt.owned_wsl_unregister_called -and $receipt.owned_wsl_unregister_exit -ne 0) -or -not $receipt.owned_wsl_registration_absent) { throw 'Owned WSL CLI cleanup or post-list absence proof failed' }
    $receipt.owned_listener_ports_closed = (@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 -ErrorAction SilentlyContinue).Count -eq 0)
    if (-not $receipt.owned_listener_ports_closed) { $failures.Add('Owned listener ports remain; no other process will be killed') }
  }
} catch {
  $failures.Add('WSL cleanup error type: ' + $_.Exception.GetType().FullName)
  if ($owned.runtime_start_attempted) {
    $owned.zero_job_teardown_proven=$false; $owned.violation_observed=$true
    $owned | ConvertTo-Json | Set-Content -LiteralPath $ownershipPath -Encoding UTF8
    $receipt.further_teardown_blocked=$true
  }
}
try {
  if ($receipt.owned_wsl_registration_absent -and (-not $owned.runtime_start_attempted -or ($owned.zero_job_teardown_proven -and -not $owned.violation_observed))) {
    foreach ($name in @('build','owned-distro','debian-rootfs.tar.gz','receiver.log','frontend.log')) {
      $path = Join-Path $Work $name
      if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
    }
  }
} catch { $failures.Add('Owned scratch cleanup error type: ' + $_.Exception.GetType().FullName) }
$mainReceipt = Join-Path $proof 'windows-local-acceptance-receipt.json'
if (Test-Path -LiteralPath $mainReceipt) {
  $main = Get-Content -LiteralPath $mainReceipt -Raw | ConvertFrom-Json
  $receipt.original_native_printers_preserved_by_main = $main.original_native_printers_preserved
  $receipt.native_default_preserved_by_main = $main.native_default_unchanged
  if ($owned.queue_absent_before -and (-not $main.original_native_printers_preserved -or -not $main.native_default_unchanged)) { $failures.Add('Original native printer/default preservation was not established; no acceptance claim') }
} else {
  $failures.Add('Main receipt missing; cleanup can recover resources but cannot assert native acceptance/default preservation')
}
$receipt.failures = @($failures)
$receipt | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $proof 'always-cleanup-receipt.json') -Encoding UTF8
if ($failures.Count -gt 0) { throw 'Owned recovery or preservation proof failed; see the sanitized cleanup receipt' }
