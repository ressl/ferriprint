# SPDX-License-Identifier: AGPL-3.0-only
# Disposable hosted Windows VM only. Builds the checked-out Ferriprint source in
# an owned WSL1 distro, exposes it on literal loopback, then exercises the real
# native Windows IPP queue: automatic Add via the inbox Microsoft IPP Class
# Driver and native PrintCapabilities/PrintTicket. Zero document/job APIs.
param([Parameter(Mandatory=$true)][string]$Work)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$proof = Join-Path $Work 'proof'
New-Item -ItemType Directory -Path $proof -Force | Out-Null
$distro = 'ferriprint-local-qa-' + $env:GITHUB_RUN_ID
$queueName = 'Ferriprint WIN NATIVE CAPS QA ' + $env:GITHUB_RUN_ID
$baseUrl = 'http://127.0.0.1:18634'
$printerUri = 'ipp://127.0.0.1:18634/ipp/print/win-native-caps'
$transportUrl = $baseUrl + '/ipp/print/win-native-caps'
$adminSecret = $null
$handler = [Net.Http.HttpClientHandler]::new()
$handler.UseProxy = $false
$handler.AllowAutoRedirect = $false
$http = [Net.Http.HttpClient]::new($handler)
$http.Timeout = [TimeSpan]::FromSeconds(10)
$receipt = [ordered]@{
  boundary = 'One disposable hosted Windows VM; Ferriprint frontend built from the exact checked-out revision in owned WSL1; literal loopback; native Windows IPP queue, capabilities and PrintTicket; zero document/print APIs.'
  frontend_under_test = 'source build of the workflow checkout, not a published release artifact'
  native_ipp_credential_flow_exercised = $false
  functional_native_queue_authentication_mode = 'Explicit anonymous grant restricted to literal loopback only. Not positive native IPP-driver authentication evidence.'
  source_commit = $env:QA_SOURCE_SHA
  document_or_print_api_calls = 0
}
$ownership = [ordered]@{
  queue_name = $queueName
  distro_name = $distro
  queue_absent_before = $false
  distro_absent_before = $false
  owned_port_name = $null
  owned_port_absent_before = $false
  inbox_ipp_registered_before = $null
  runtime_start_attempted = $false
  zero_job_teardown_proven = $false
  violation_observed = $false
}
function Save-OwnedState {
  $ownership | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $proof 'ownership.json') -Encoding UTF8
}
Save-OwnedState
function Invoke-Wsl([string[]]$Arguments) {
  & wsl.exe @Arguments
  if ($LASTEXITCODE -ne 0) { throw ('Owned WSL command failed with exit ' + $LASTEXITCODE) }
}
function Invoke-WslSecretInput([string[]]$Arguments, [string]$Secret) {
  $start = New-Object Diagnostics.ProcessStartInfo
  $start.FileName = (Join-Path $env:SystemRoot 'System32\wsl.exe')
  $start.UseShellExecute = $false
  $start.RedirectStandardInput = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
  $process = [Diagnostics.Process]::Start($start)
  $outputTask = $process.StandardOutput.ReadToEndAsync()
  $errorTask = $process.StandardError.ReadToEndAsync()
  $process.StandardInput.WriteLine($Secret)
  $process.StandardInput.Close()
  if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Bounded QA initialization deadline exceeded' }
  $output = $outputTask.GetAwaiter().GetResult()
  $errors = $errorTask.GetAwaiter().GetResult()
  if ($process.ExitCode -ne 0) {
    $receipt.initialization_failure_diagnostics = [ordered]@{
      exit_code = [int]$process.ExitCode
      stdout_utf16_code_units = $output.Length
      stderr_utf16_code_units = $errors.Length
    }
    throw ('QA initialization failed, exit ' + $process.ExitCode)
  }
  $process.Dispose()
}
function New-QaSecret {
  $bytes = New-Object byte[] 32
  [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
  [Convert]::ToBase64String($bytes)
}
function Invoke-QaRequest([string]$Method, [string]$Path, [string]$User, [string]$Password, $Form) {
  $request = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::new($Method), $baseUrl + $Path)
  try {
    if ($User) {
      $basic = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($User + ':' + $Password))
      $request.Headers.Authorization = [Net.Http.Headers.AuthenticationHeaderValue]::new('Basic', $basic)
    }
    if ($null -ne $Form) {
      $pairs = [Collections.Generic.List[Collections.Generic.KeyValuePair[string,string]]]::new()
      foreach ($entry in $Form.GetEnumerator()) { $pairs.Add([Collections.Generic.KeyValuePair[string,string]]::new($entry.Key, [string]$entry.Value)) }
      $request.Content = [Net.Http.FormUrlEncodedContent]::new($pairs)
      $request.Headers.Add('Origin', $baseUrl)
    }
    $response = $http.SendAsync($request).GetAwaiter().GetResult()
    try { [pscustomobject]@{ status=[int]$response.StatusCode; text=$response.Content.ReadAsStringAsync().GetAwaiter().GetResult() } }
    finally { $response.Dispose() }
  } finally { $request.Dispose() }
}
function Get-QaCsrf {
  $page = Invoke-QaRequest 'GET' '/admin/users' 'qa-admin' $adminSecret $null
  if ($page.status -ne 200) { throw 'QA administrator read failed' }
  $match = [regex]::Match($page.text, 'name="csrf" value="([^"]+)"')
  if (-not $match.Success) { throw 'Supported QA account form CSRF missing' }
  $match.Groups[1].Value
}
function Enable-QaAnonymousGrant {
  [void](Get-QaCsrf)
  $grant = Invoke-QaRequest 'POST' '/admin/anonymous' 'qa-admin' $adminSecret @{ csrf=(Get-QaCsrf); printer='win-native-caps' }
  if ($grant.status -ne 200) { throw 'Explicit loopback-only anonymous functional grant failed' }
}
try {
  . (Join-Path $Work 'windows-wsl-metadata.ps1')
  $os = Get-CimInstance Win32_OperatingSystem
  $receipt.windows = [ordered]@{ caption=$os.Caption; version=$os.Version; build=$os.BuildNumber; architecture=$os.OSArchitecture }
  $receipt.spooler_before = (Get-Service Spooler).Status.ToString()
  if ($receipt.spooler_before -ne 'Running') { throw 'Existing Spooler is not running; no service start is authorized' }
  if (-not (Get-Command Add-Printer).Parameters.ContainsKey('IppURL')) { throw 'Native Add-Printer IppURL missing' }
  $inboxDriver = @(Get-PrinterDriver | Where-Object Name -eq 'Microsoft IPP Class Driver')
  $ownership.inbox_ipp_registered_before = ($inboxDriver.Count -gt 0)
  $receipt.inbox_ipp_registered_before = $ownership.inbox_ipp_registered_before
  Save-OwnedState
  $wslPackages = @(Get-AppxPackage -AllUsers -Name MicrosoftCorporationII.WindowsSubsystemForLinux)
  if ($wslPackages.Count -ne 1) { throw 'Required pre-existing WSL package unavailable; no install/update/repair is authorized' }
  $receipt.preexisting_wsl = [ordered]@{ name=$wslPackages[0].Name; version=$wslPackages[0].Version.ToString(); status=$wslPackages[0].Status.ToString() }
  if (@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Owned loopback ports already have listeners; no reuse/reset/kill is authorized' }
  $printerBaseline = @(Get-Printer | Select-Object Name,DriverName,PortName)
  $portBaseline = @(Get-PrinterPort | Select-Object -ExpandProperty Name)
  $defaultBefore = @(Get-CimInstance Win32_Printer | Where-Object Default | Select-Object -ExpandProperty Name)
  $receipt.existing_native_printer_count = $printerBaseline.Count
  if (Get-Printer -Name $queueName -ErrorAction SilentlyContinue) { throw 'Owned temporary queue name already exists' }
  $nativeNameAbsentConfirmed = $true
  $ownership.queue_absent_before = $true
  Save-OwnedState
  foreach ($feature in @('Microsoft-Windows-Subsystem-Linux')) {
    if ((Get-WindowsOptionalFeature -Online -FeatureName $feature).State -ne 'Enabled') { throw 'Required pre-existing WSL feature unavailable' }
  }
  $rootfs = Join-Path $Work 'debian-rootfs.tar.gz'
  $anonymousRegistry = Invoke-RestMethod 'https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/debian:pull'
  Invoke-WebRequest -Uri 'https://registry-1.docker.io/v2/library/debian/blobs/sha256:11422d7ca1a44390a32bd8d8d9dc5946b4d053b3bbd9507d682e35584ba84a26' -Headers @{ Authorization=('Bearer ' + $anonymousRegistry.token) } -OutFile $rootfs
  $anonymousRegistry = $null
  if ((Get-FileHash -LiteralPath $rootfs -Algorithm SHA256).Hash.ToLowerInvariant() -ne '11422d7ca1a44390a32bd8d8d9dc5946b4d053b3bbd9507d682e35584ba84a26') { throw 'Rootfs checksum mismatch' }
  $before = Get-FerriprintWslState -DistributionName $distro
  $receipt.owned_wsl_before_import = $before
  if (-not $before.registration_observed) { throw 'Native WSL CLI registration could not be observed; no absence assumption' }
  if ($before.registered) { throw 'Owned WSL distro name already exists; no reuse/reset' }
  $ownership.distro_absent_before = $true
  Save-OwnedState
  Invoke-Wsl -Arguments @('--import',$distro,(Join-Path $Work 'owned-distro'),$rootfs,'--version','1')
  $configuration = Get-FerriprintWslState -DistributionName $distro
  $receipt.owned_wsl_configuration = $configuration
  if (-not $configuration.registration_observed -or -not $configuration.registered -or -not $configuration.version_observed -or $configuration.version -ne 1) { throw 'Native WSL CLI did not observe the exact owned WSL1 instance' }
  $linuxWork = ((& wsl.exe -d $distro --exec wslpath -a -u $Work) | Select-Object -First 1).Trim()
  if (-not $linuxWork.StartsWith('/mnt/')) { throw 'Owned work path is not inside the observed /mnt namespace' }
  $receipt.linux_work = $linuxWork
  $buildEnv = 'QA_SOURCE_SHA=' + $env:QA_SOURCE_SHA
  Invoke-Wsl -Arguments @('-d',$distro,'--exec','env',$buildEnv,'bash',($linuxWork+'/win-native-build.sh'),$linuxWork,'build')
  $receipt.built_revision = (Invoke-Wsl -Arguments @('-d',$distro,'--exec','cat',"$linuxWork/proof/built-git-revision.txt")) -join ''
  $receipt.built_binaries = (Invoke-Wsl -Arguments @('-d',$distro,'--exec','cat',"$linuxWork/proof/built-binaries.sha256")) -join "`n"
  $adminSecret = New-QaSecret
  Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/win-native-build.sh'),$linuxWork,'start-receiver')
  $receiverReady = $false
  for ($attempt=0; $attempt -lt 20; $attempt++) {
    try { $null = Invoke-RestMethod 'http://127.0.0.1:18631/audit' -NoProxy -TimeoutSec 2; $receiverReady=$true; break } catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $receiverReady) { throw 'Owned loopback receiver readiness deadline exceeded' }
  $ownership.runtime_start_attempted = $true
  Save-OwnedState
  Invoke-WslSecretInput -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/win-native-build.sh'),$linuxWork,'install-and-start') -Secret $adminSecret
  $frontendReady = $false
  for ($attempt=0; $attempt -lt 40; $attempt++) {
    try {
      $health = Invoke-WebRequest ($baseUrl+'/health') -NoProxy -TimeoutSec 2
      if ($health.StatusCode -eq 200) { $frontendReady=$true; break }
    } catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $frontendReady) { throw 'Built Ferriprint loopback readiness deadline exceeded' }
  $listeners = @(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 | Select-Object LocalAddress,LocalPort)
  if ($listeners.Count -ne 2 -or @($listeners | Where-Object LocalAddress -ne '127.0.0.1').Count -ne 0) { throw 'Owned runtime is not exactly the two literal-loopback listeners' }
  $receipt.listeners = $listeners
  Enable-QaAnonymousGrant
  Add-Printer -Name $queueName -IppURL $transportUrl
  $actualQueue = Get-Printer -Name $queueName
  $ownership.owned_port_name = $actualQueue.PortName
  $ownership.owned_port_absent_before = ($portBaseline -notcontains $actualQueue.PortName)
  Save-OwnedState
  $receipt.actual_native_queue = [ordered]@{ driver=$actualQueue.DriverName; port=$actualQueue.PortName; ipp_transport_url=$transportUrl; advertised_ipp_uri=$printerUri }
  if ($actualQueue.DriverName -ne 'Microsoft IPP Class Driver') { throw 'Automatic native Add did not select the pre-existing Microsoft IPP Class Driver' }
  & (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -NoProfile -NonInteractive -File (Join-Path $Work 'win-native-capabilities.ps1') -QueueName $queueName -ProofDirectory $proof
  $capabilitiesExit = $LASTEXITCODE
  $receipt.native_capabilities_exit = $capabilitiesExit
  Invoke-Wsl -Arguments @('-d',$distro,'--exec','cp',"$linuxWork/frontend.log",(Join-Path $Work 'proof/frontend.log'))
  if ($capabilitiesExit -ne 0) { throw 'Native PrintCapabilities/PrintTicket did not preserve the required semantics; exact driver response retained in proof' }
  $receipt.native_local_job_count = @(Get-PrintJob -PrinterName $queueName).Count
  if ($receipt.native_local_job_count -ne 0) { $ownership.violation_observed=$true; Save-OwnedState }
  $receipt.actual_receiver_audit = Invoke-RestMethod 'http://127.0.0.1:18631/audit' -NoProxy
  if ($receipt.native_local_job_count -ne 0) { $ownership.violation_observed=$true; Save-OwnedState; throw 'Unexpected native job' }
  $receipt.zero_job_teardown_proven = $true
  $ownership.zero_job_teardown_proven = $true
  Save-OwnedState
} finally {
  try {
    if ($nativeNameAbsentConfirmed) {
      $ownedQueue = Get-Printer -Name $queueName -ErrorAction SilentlyContinue
      if ($null -ne $ownedQueue) {
        $ownJobs = @(Get-PrintJob -PrinterName $queueName)
        if ($ownJobs.Count -eq 0) {
          $ownedPort = $ownedQueue.PortName
          Remove-Printer -Name $queueName
          if (($portBaseline -notcontains $ownedPort) -and -not (Get-Printer | Where-Object PortName -eq $ownedPort)) {
            if (Get-PrinterPort -Name $ownedPort -ErrorAction SilentlyContinue) { Remove-PrinterPort -Name $ownedPort }
          }
        } else {
          $receipt.unexpected_native_jobs_preserved = $ownJobs.Count
          throw 'Unexpected native jobs preserved with fixture state; no teardown or cancellation'
        }
      }
      if ($ownership.distro_absent_before) {
        try {
          $state = Get-FerriprintWslState -DistributionName $distro
          if ($state.registration_observed -and $state.registered) {
            & wsl.exe --terminate $distro
            & wsl.exe --unregister $distro
            $receipt.owned_wsl_unregister_exit = $LASTEXITCODE
          }
          $after = Get-FerriprintWslState -DistributionName $distro
          $receipt.owned_wsl_after_teardown = $after
          $receipt.owned_wsl_unregistered = ($after.registration_observed -and -not $after.registered)
        } catch { $receipt.owned_wsl_unregister_error = $true }
      }
    }
  } finally {
    $adminSecret = $null
    $http.Dispose()
    $receipt.spooler_after = (Get-Service Spooler).Status.ToString()
    $receipt.owned_loopback_listener_ports_closed = (@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 -ErrorAction SilentlyContinue).Count -eq 0)
    $receipt | ConvertTo-Json -Depth 10 -EscapeHandling EscapeNonAscii | Set-Content -LiteralPath (Join-Path $proof 'windows-local-acceptance-receipt.json') -Encoding UTF8
  }
}
