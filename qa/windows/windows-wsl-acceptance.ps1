# SPDX-License-Identifier: AGPL-3.0-only
# PLAN SOURCE ONLY. Import/setup/queue are forbidden until Parent approves this complete workflow.
param([Parameter(Mandatory=$true)][string]$Work)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$proof = Join-Path $Work 'proof'
New-Item -ItemType Directory -Path $proof -Force | Out-Null
$distro = 'ferriprint-local-qa-' + $env:GITHUB_RUN_ID
$queueName = 'Ferriprint LOCAL NONPRINTING QA ' + $env:GITHUB_RUN_ID
$baseUrl = 'http://127.0.0.1:18634'
$printerUri = 'ipp://127.0.0.1:18634/ipp/print/windows-local-qa'
$transportUrl = $baseUrl + '/ipp/print/windows-local-qa'
$adminSecret = $null
$nativeSecret = $null
$observerAttempted = $false
$observerDisarmed = $false
$restartTransitionUncertain = $false
$handler = [Net.Http.HttpClientHandler]::new()
$handler.UseProxy = $false
$handler.AllowAutoRedirect = $false
$http = [Net.Http.HttpClient]::new($handler)
$http.Timeout = [TimeSpan]::FromSeconds(10)
$receipt = [ordered]@{
  boundary = 'One disposable hosted Windows VM only; actual published Ferriprint Linux ELF in owned WSL1; literal shared loopback runtime; native Windows IPP queue/capabilities/PrintTicket; zero document/print APIs.'
  native_ipp_credential_flow_exercised = $false
  functional_native_queue_authentication_mode = 'Explicit anonymous grant restricted to literal loopback only. This is not positive native IPP-driver authentication evidence.'
  separate_http_account_authentication = $null
  tls_exercised = $false
  source_commit = '503a7b90bc38a5d8b6917788ff7e237b2e684216'
  published_frontend_elf_sha256 = '473767d296f1cc2f86d0f84312da31f4aa031e195437ef7f5d348ad3f5e0bee6'
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
  restart_attempted = $false
  restart_state_uncertain = $false
}
function Save-OwnedState {
  $ownership | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $proof 'ownership.json') -Encoding UTF8
}
Save-OwnedState
function Assert-FileHash([string]$Path, [string]$Expected) {
  if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Expected) { throw ('Artifact checksum mismatch: ' + [IO.Path]::GetFileName($Path)) }
}
function Invoke-Wsl([string[]]$Arguments) {
  & wsl.exe @Arguments
  if ($LASTEXITCODE -ne 0) { throw ('Owned WSL command failed with exit ' + $LASTEXITCODE) }
}
function Invoke-WslSecretInput([string[]]$Arguments, [string]$Secret, [switch]$VersionControl) {
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
  if (-not $VersionControl) { $process.StandardInput.WriteLine($Secret) }
  $process.StandardInput.Close()
  if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Bounded QA initialization deadline exceeded' }
  $output = $outputTask.GetAwaiter().GetResult()
  $errors = $errorTask.GetAwaiter().GetResult()
  $contextIds = @()
  $errnoCodes = @()
  if ($process.ExitCode -ne 0) {
    # Fixed-label classifier only; the pre-secret control has the sole bounded-text exception below.
    $knownContexts = [ordered]@{
      configuration_read = 'read configuration '
      configuration_parse = 'parse configuration'
      configuration_legacy = 'legacy bootstrap users/printers require the explicit migrate-0.2 command'
      password_confirmation = 'passwords differ'
      password_stdin_limit = 'password input too long'
      password_stdin_utf8 = 'password input must be UTF-8'
      password_length = 'password must contain 12..1024 bytes'
      password_hash = 'password hashing failed'
      state_privacy = 'state directory must be owner-only'
      state_lock = 'state directory is already in use; stop the service before offline changes'
      managed_create = 'create new managed file without overwrite'
      managed_acl = 'new managed file has an unsafe inherited ACL; no credentials written'
      managed_owner_assign = 'assign new managed file to existing state owner'
      managed_owner_required = 'run offline initialization as the state owner (or root)'
      runuser_nonroot = 'may not be used by non-root users'
      runuser_pam_session = 'cannot open session:'
      runuser_groups = 'cannot set groups'
      runuser_uid = 'cannot set user id'
      runuser_exec = 'failed to execute '
      elf_shared_library = 'error while loading shared libraries'
      cli_unknown_argument = 'unexpected argument '
      cli_required_argument = 'the following required arguments were not provided:'
      cli_invalid_value = 'invalid value '
    }
    $contextIds = @(
      foreach ($entry in $knownContexts.GetEnumerator()) {
        if ($output.Contains([string]$entry.Value, [StringComparison]::Ordinal) -or $errors.Contains([string]$entry.Value, [StringComparison]::Ordinal)) { [string]$entry.Key }
      }
    )
    $errnoCodes = @(
      foreach ($errno in @(1,2,4,5,6,8,9,11,12,13,17,20,21,22,23,24,25,28,30,32,36,38,40,95,122)) {
        $marker = '(os error ' + $errno + ')'
        if ($output.Contains($marker, [StringComparison]::Ordinal) -or $errors.Contains($marker, [StringComparison]::Ordinal)) { [int]$errno }
      }
    )
    if (-not $VersionControl) {
      $receipt.initialization_failure_diagnostics = [ordered]@{
        exit_code = [int]$process.ExitCode
        stdout_utf16_code_units = $output.Length
        stderr_utf16_code_units = $errors.Length
        recognized_context_ids = $contextIds
        recognized_errno = $errnoCodes
      }
      throw ('QA initialization failed, exit ' + $process.ExitCode)
    }
  }
  if ($VersionControl) {
    # Public --version canary only: snapshot before any credential/PHC/catalog creation.
    $receipt.launch_control = [ordered]@{
      exit_code = [int]$process.ExitCode
      exact_version_match = ($process.ExitCode -eq 0 -and ($output -ceq "ferriprint 0.3.0`n" -or $output -ceq "ferriprint 0.3.0`r`n"))
      stdout_utf16_code_units = $output.Length
      stderr_utf16_code_units = $errors.Length
      stdout_text = $output.Substring(0, [Math]::Min(4096, $output.Length))
      stdout_text_truncated = ($output.Length -gt 4096)
      stderr_text = $errors.Substring(0, [Math]::Min(4096, $errors.Length))
      stderr_text_truncated = ($errors.Length -gt 4096)
      recognized_context_ids = $contextIds
      recognized_errno = $errnoCodes
    }
  }
  # Password input and raw initialization streams are never logged or archived.
  $process.Dispose()
}
function New-QaSecret {
  $bytes = New-Object byte[] 32
  [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
  [Convert]::ToBase64String($bytes)
}
function Invoke-QaRequest([string]$Method, [string]$Path, [string]$User, [string]$Password, $Form, [byte[]]$IppBody) {
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
    } elseif ($null -ne $IppBody) {
      $request.Content = [Net.Http.ByteArrayContent]::new($IppBody)
      $request.Content.Headers.ContentType = [Net.Http.Headers.MediaTypeHeaderValue]::new('application/ipp')
    }
    $response = $http.SendAsync($request).GetAwaiter().GetResult()
    try { [pscustomobject]@{ status=[int]$response.StatusCode; bytes=$response.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult() } }
    finally { $response.Dispose() }
  } finally { $request.Dispose() }
}
function Get-QaCsrf {
  $page = Invoke-QaRequest 'GET' '/admin/users' 'qa-admin' $adminSecret $null $null
  if ($page.status -ne 200) { throw 'QA administrator read failed' }
  $match = [regex]::Match([Text.Encoding]::UTF8.GetString($page.bytes), 'name="csrf" value="([^"]+)"')
  if (-not $match.Success) { throw 'Supported QA account form CSRF missing' }
  $match.Groups[1].Value
}
function New-GpaBody([string]$Uri) {
  $stream = [IO.MemoryStream]::new()
  try {
    $header = [byte[]](2,0,0,11,0,0,0,1,1)
    $stream.Write($header, 0, $header.Length)
    foreach ($attribute in @(@(71,'attributes-charset','utf-8'), @(72,'attributes-natural-language','en'), @(69,'printer-uri',$Uri), @(68,'requested-attributes','all'))) {
      $name = [Text.Encoding]::UTF8.GetBytes($attribute[1]); $value = [Text.Encoding]::UTF8.GetBytes($attribute[2])
      $stream.WriteByte([byte]$attribute[0])
      $stream.WriteByte([byte]($name.Length -shr 8)); $stream.WriteByte([byte]($name.Length -band 255)); $stream.Write($name, 0, $name.Length)
      $stream.WriteByte([byte]($value.Length -shr 8)); $stream.WriteByte([byte]($value.Length -band 255)); $stream.Write($value, 0, $value.Length)
    }
    $stream.WriteByte(3)
    ,$stream.ToArray()
  } finally { $stream.Dispose() }
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
  if ($wslPackages.Count -ne 1 -or $wslPackages[0].Status.ToString() -ne 'Ok') { throw 'Required pre-existing healthy WSL package unavailable; no install/update/repair is authorized' }
  $receipt.preexisting_wsl = [ordered]@{ name=$wslPackages[0].Name; version=$wslPackages[0].Version.ToString(); status=$wslPackages[0].Status.ToString(); executable_version=(Get-Item (Join-Path $env:SystemRoot 'System32\wsl.exe')).VersionInfo.FileVersion }
  if (@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Owned loopback ports already have listeners; no reuse/reset/kill is authorized' }
  $printerBaseline = @(Get-Printer | Select-Object Name,DriverName,PortName)
  $portBaseline = @(Get-PrinterPort | Select-Object -ExpandProperty Name)
  $defaultBefore = @(Get-CimInstance Win32_Printer | Where-Object Default | Select-Object -ExpandProperty Name)
  $receipt.existing_native_printer_count = $printerBaseline.Count
  $receipt.existing_native_active_job_count = @($printerBaseline | ForEach-Object { Get-PrintJob -PrinterName $_.Name }).Count
  if ($receipt.existing_native_active_job_count -ne 0) { throw 'Fresh native job baseline is not zero; no cancel or replay is authorized' }
  if (Get-Printer -Name $queueName -ErrorAction SilentlyContinue) { throw 'Owned temporary queue name already exists' }
  $nativeNameAbsentConfirmed = $true
  $ownership.queue_absent_before = $true
  Save-OwnedState
  $receipt.owned_wsl_before_import = Get-FerriprintWslState -DistributionName $distro
  $receipt.owned_wsl_before_import | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $proof 'owned-wsl-before-import.json') -Encoding UTF8
  if (-not $receipt.owned_wsl_before_import.registration_observed) { throw 'Native WSL CLI registration could not be observed; no absence assumption or setup fallback' }
  if ($receipt.owned_wsl_before_import.registered) { throw 'Owned WSL name already exists; no reuse/reset' }
  $ownership.distro_absent_before = $true
  Save-OwnedState
  foreach ($feature in @('Microsoft-Windows-Subsystem-Linux')) {
    if ((Get-WindowsOptionalFeature -Online -FeatureName $feature).State -ne 'Enabled') { throw 'Required pre-existing WSL feature unavailable; no feature enable/repair is authorized' }
  }
  $rootfs = Join-Path $Work 'debian-rootfs.tar.gz'
  $anonymousRegistry = Invoke-RestMethod 'https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/debian:pull'
  Invoke-WebRequest -Uri 'https://registry-1.docker.io/v2/library/debian/blobs/sha256:11422d7ca1a44390a32bd8d8d9dc5946b4d053b3bbd9507d682e35584ba84a26' -Headers @{ Authorization=('Bearer ' + $anonymousRegistry.token) } -OutFile $rootfs
  $anonymousRegistry = $null
  Assert-FileHash $rootfs '11422d7ca1a44390a32bd8d8d9dc5946b4d053b3bbd9507d682e35584ba84a26'
  Invoke-Wsl -Arguments @('--import',$distro,(Join-Path $Work 'owned-distro'),$rootfs,'--version','1')
  $receipt.owned_wsl_configuration = Get-FerriprintWslState -DistributionName $distro
  $receipt.owned_wsl_configuration | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $proof 'owned-wsl-configuration.json') -Encoding UTF8
  if (-not $receipt.owned_wsl_configuration.registration_observed -or -not $receipt.owned_wsl_configuration.registered -or -not $receipt.owned_wsl_configuration.version_observed -or $receipt.owned_wsl_configuration.version -ne 1) { throw 'Native WSL CLI did not observe the exact owned WSL1 instance; no missing-version assumption or WSL2 fallback' }
  $receipt.actual_wsl_version = $receipt.owned_wsl_configuration.version
  $linuxPathOutput = @(& wsl.exe -d $distro --exec wslpath -a -u $Work)
  $linuxPathExit = $LASTEXITCODE
  $expectedWorkName = 'ferriprint-windows-local-qa-' + $env:GITHUB_RUN_ID
  $singlePathLine = ($linuxPathOutput.Count -eq 1 -and $linuxPathOutput[0] -is [string] -and $linuxPathOutput[0].Length -gt 0 -and $linuxPathOutput[0] -notmatch '[\x00-\x1f\x7f]')
  $ownedPathLine = ($singlePathLine -and $linuxPathOutput[0].StartsWith('/mnt/',[StringComparison]::Ordinal) -and [IO.Path]::GetFileName($Work) -eq $expectedWorkName -and $linuxPathOutput[0].EndsWith('/'+$expectedWorkName,[StringComparison]::Ordinal))
  $receipt.owned_work_path_translation = [ordered]@{
    exit_code = $linuxPathExit
    output_line_count = $linuxPathOutput.Count
    single_string_line = $singlePathLine
    owned_namespace_matches = $ownedPathLine
    exact_owned_output_before_trim = $(if ($ownedPathLine) { $linuxPathOutput[0] } else { $null })
  }
  $receipt.owned_work_path_translation | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $proof 'owned-work-path-translation.json') -Encoding UTF8
  if ($linuxPathExit -ne 0) { throw ('Native wslpath failed with exit ' + $linuxPathExit + '; no mounted path is inferred') }
  if ($linuxPathOutput.Count -ne 1) { throw ('Native wslpath returned ' + $linuxPathOutput.Count + ' stdout lines; expected exactly one owned path') }
  if (-not $singlePathLine) { throw 'Native wslpath did not return a nonempty control-free string line; no path is inferred' }
  if (-not $ownedPathLine) { throw 'Native wslpath output is outside the observed /mnt/ owned-work namespace; no path is inferred' }
  $linuxWork = $linuxPathOutput[0].Trim()
  Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'build')
  # Secret-free control must pass before generating credentials or invoking real init.
  Invoke-WslSecretInput -Arguments @('-d',$distro,'--exec','/sbin/runuser','-u','ferriprintqa','--','/opt/ferriprint-windows-local-qa/ferriprint','--config','/opt/ferriprint-windows-local-qa/ferriprint.toml','--version') -VersionControl
  if (-not $receipt.launch_control.exact_version_match) { throw 'Published frontend version control failed; no runtime fallback is authorized' }
  $adminSecret = New-QaSecret
  $nativeSecret = New-QaSecret
  Invoke-WslSecretInput -Arguments @('-d',$distro,'--exec','/sbin/runuser','-u','ferriprintqa','--','/opt/ferriprint-windows-local-qa/ferriprint','--config','/opt/ferriprint-windows-local-qa/ferriprint.toml','user','init','--username','qa-admin') -Secret $adminSecret
  # Persist conservatively before the first runtime launch, including a failed launch attempt.
  $ownership.runtime_start_attempted = $true
  Save-OwnedState
  Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'start-receiver')
  $receiverReady = $false
  for ($attempt=0; $attempt -lt 20; $attempt++) {
    try { $receiverAudit = Invoke-RestMethod 'http://127.0.0.1:18631/audit' -NoProxy -TimeoutSec 2; $receiverReady=$true; break } catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $receiverReady) { throw 'Owned loopback receiver readiness deadline exceeded; no tunnel/network fallback' }
  Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'install-and-start')
  $frontendReady = $false
  for ($attempt=0; $attempt -lt 20; $attempt++) {
    try { $health = Invoke-WebRequest ($baseUrl+'/health') -NoProxy -TimeoutSec 2; if ($health.StatusCode -eq 200) { $frontendReady=$true; break } } catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $frontendReady) { throw 'Actual Ferriprint loopback readiness deadline exceeded; no exposure/network fallback' }
  $receipt.runtime_provenance = @(Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'runtime-proof'))
  $listeners = @(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 | Select-Object LocalAddress,LocalPort)
  if ($listeners.Count -ne 2 -or @($listeners | Where-Object LocalAddress -ne '127.0.0.1').Count -ne 0 -or @($listeners.LocalPort | Sort-Object -Unique).Count -ne 2) { throw 'Owned runtime is not exactly the two literal-loopback-only listeners; no network widening is authorized' }
  $receipt.actual_literal_loopback_listeners = $listeners
  $create = Invoke-QaRequest 'POST' '/admin/users' 'qa-admin' $adminSecret @{csrf=(Get-QaCsrf);intent='create';username='native-qa';role='user';password=$nativeSecret;password_confirm=$nativeSecret;printer='windows-local-qa'} $null
  if ($create.status -ne 200) { throw 'Supported fresh QA user creation failed' }
  $anonymousGrant = Invoke-QaRequest 'POST' '/admin/anonymous' 'qa-admin' $adminSecret @{csrf=(Get-QaCsrf);printer='windows-local-qa'} $null
  if ($anonymousGrant.status -ne 200) { throw 'Explicit loopback-only anonymous functional grant failed' }
  $regular = Invoke-QaRequest 'GET' '/api/printers' 'native-qa' $nativeSecret $null $null
  $forbidden = Invoke-QaRequest 'GET' '/admin/users' 'native-qa' $nativeSecret $null $null
  if ($regular.status -ne 200 -or $forbidden.status -ne 403) { throw 'Separate native Windows HTTP account authentication/role proof failed' }
  $receipt.separate_http_account_authentication = [ordered]@{ regular_printers_status=$regular.status; regular_admin_status=$forbidden.status; native_ipp_driver_credential_claim=$false }
  $gpa = Invoke-QaRequest 'POST' '/ipp/print/windows-local-qa' '' '' $null (New-GpaBody $printerUri)
  if ($gpa.status -ne 200 -or $gpa.bytes.Length -lt 8 -or $gpa.bytes[2] -ne 0 -or $gpa.bytes[3] -ne 0) { throw 'Actual published Ferriprint GPA did not return successful-ok' }
  $gpaText = [Text.Encoding]::UTF8.GetString($gpa.bytes)
  foreach ($recipe in @('a3-borderless-high','a4-normal','a4-duplex')) { if (-not $gpaText.Contains($recipe)) { throw 'Actual Ferriprint saved recipe collection missing before native Add' } }
  if (-not $gpaText.Contains('urn:uuid:85b0b6cf-0e3a-5952-822a-7c73e569aab2')) { throw 'Actual loopback Ferriprint UUID mismatch before native Add' }
  [IO.File]::WriteAllBytes((Join-Path $proof 'actual-ferriprint-gpa.ipp'),$gpa.bytes)
  $jobsBefore = Invoke-QaRequest 'GET' '/api/jobs' 'qa-admin' $adminSecret $null $null
  if ($jobsBefore.status -ne 200 -or @([Text.Encoding]::UTF8.GetString($jobsBefore.bytes) | ConvertFrom-Json).Count -ne 0) { throw 'Fresh frontend ledger is not empty' }
  # Authenticated HTTP/init and the direct control GPA are outside observation.
  $receipt.pre_restart_runtime_provenance = $receipt.runtime_provenance
  $observerDisarmed = $false
  try {
    $restartTransitionUncertain=$true
    $ownership.restart_attempted=$true; $ownership.restart_state_uncertain=$true; Save-OwnedState
    try {
      if ($null -ne (Get-Printer | Where-Object Name -eq $queueName)) { throw 'Owned native queue unexpectedly exists before startup observation' }
      $restartNativeJobs=0
      foreach ($existing in @(Get-Printer)) { $restartNativeJobs += @(Get-PrintJob -PrinterName $existing.Name).Count }
      $restartAudit=Invoke-RestMethod 'http://127.0.0.1:18631/audit' -NoProxy
      if ($restartNativeJobs -ne 0 -or $restartAudit.refused_operations -ne 0 -or @($restartAudit.jobs).Count -ne 0) { throw 'Fresh owned restart counters are not zero' }
      $receipt.pre_restart_native_jobs=$restartNativeJobs
      $receipt.pre_restart_receiver_audit=$restartAudit
      $frontendReady=$false
      $oldRuntimePid=@($receipt.pre_restart_runtime_provenance | Where-Object { $_ -match '^473767d296f1cc2f86d0f84312da31f4aa031e195437ef7f5d348ad3f5e0bee6  /proc/[0-9]+/exe$' })
      if ($oldRuntimePid.Count -ne 1) { throw 'Owned old actual proc proof missing before quiescence' }
      $oldPidMatch=[regex]::Match($oldRuntimePid[0],'/proc/([0-9]+)/exe$')
      if (-not $oldPidMatch.Success) { throw 'Owned old process identity unavailable' }
      $receipt.pre_restart_frontend_pid=[uint32]$oldPidMatch.Groups[1].Value
      Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'observer-quiesce',([string]$receipt.pre_restart_frontend_pid))
      $restartListeners=@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 | Select-Object LocalAddress,LocalPort)
      if ($restartListeners.Count -ne 1 -or $restartListeners[0].LocalAddress -ne '127.0.0.1' -or $restartListeners[0].LocalPort -ne 18631) { throw 'Owned old frontend listener closure is unproven' }
      $receipt.old_owned_frontend_listener_closed=$true
      $observerAttempted=$true
      Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'observer-arm')
      $armedObserver=Get-Content -LiteralPath (Join-Path $proof 'native-observer.json') -Raw | ConvertFrom-Json
      $restartProof=Get-Content -LiteralPath (Join-Path $proof 'native-observer-startup.json') -Raw | ConvertFrom-Json
      $receipt.owned_restart_proof=$restartProof
      if ($armedObserver.startup_identity_and_state_proven -ne $true -or $restartProof.preservation_required -ne $false -or $armedObserver.frontend_pid -eq $restartProof.old_frontend_pid) { throw 'Actual new startup identity or preserved state unproven' }
      $newRuntime=@(Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'runtime-proof'))
      $newRuntimePid=@($newRuntime | Where-Object { $_ -match '^473767d296f1cc2f86d0f84312da31f4aa031e195437ef7f5d348ad3f5e0bee6  /proc/[0-9]+/exe$' })
      if ($newRuntimePid.Count -ne 1) { throw 'New actual proc ELF proof missing' }
      $pidMatch=[regex]::Match($newRuntimePid[0],'/proc/([0-9]+)/exe$')
      if (-not $pidMatch.Success -or [uint32]$pidMatch.Groups[1].Value -ne [uint32]$armedObserver.frontend_pid) { throw 'Runtime proof refers to a different process' }
      $receipt.runtime_provenance=$newRuntime
      $newListeners=@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 | Select-Object LocalAddress,LocalPort)
      if ($newListeners.Count -ne 2 -or @($newListeners | Where-Object LocalAddress -ne '127.0.0.1').Count -ne 0 -or @($newListeners.LocalPort | Sort-Object -Unique).Count -ne 2) { throw 'Restarted fixture is not exactly the original loopback listeners' }
      $receipt.actual_literal_loopback_listeners=$newListeners
      $frontendReady=$true
      $restartTransitionUncertain=$false
      $ownership.restart_state_uncertain=$false; Save-OwnedState
    } catch {
      $ownership.violation_observed=$true; $ownership.restart_state_uncertain=$true; Save-OwnedState
      throw 'Owned restart/startup identity uncertain; preserve fixture'
    }
    # ONE public, nonauthenticating coverage canary; not native acceptance.
    $observerCanaryBody = New-GpaBody $printerUri
    $observerCanaryBody[4]=0x46; $observerCanaryBody[5]=0x50; $observerCanaryBody[6]=0x4f; $observerCanaryBody[7]=0x31
    $observerCanary = Invoke-QaRequest 'POST' '/ipp/print/windows-local-qa' '' '' $null $observerCanaryBody
    if ($observerCanary.status -ne 200) { throw 'Secret-free observer coverage canary failed; no native Add is authorized' }
    Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'observer-native')
    Add-Printer -Name $queueName -IppURL $transportUrl
    $actualQueue = Get-Printer -Name $queueName
    $ownership.owned_port_name = $actualQueue.PortName
    $ownership.owned_port_absent_before = ($portBaseline -notcontains $actualQueue.PortName)
    Save-OwnedState
    $receipt.actual_native_queue = [ordered]@{ driver=$actualQueue.DriverName; port=$actualQueue.PortName; ipp_transport_url=$transportUrl; advertised_ipp_uri=$printerUri }
    if ($actualQueue.DriverName -ne 'Microsoft IPP Class Driver') { throw 'Automatic native Add did not select the pre-existing Microsoft IPP Class Driver; no forced-driver fallback is authorized' }
    & (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -NoProfile -NonInteractive -File (Join-Path $Work 'windows-native-capabilities.ps1') -QueueName $queueName -ProofDirectory $proof
    if ($LASTEXITCODE -ne 0) { throw 'Actual native PrintCapabilities/PrintTicket API failed; no replacement client or mock fallback' }
  } finally {
    if ($restartTransitionUncertain) { $ownership.violation_observed=$true; $ownership.restart_state_uncertain=$true; Save-OwnedState }
    if ($observerAttempted) {
      try {
        Invoke-Wsl -Arguments @('-d',$distro,'--exec','bash',($linuxWork+'/windows-wsl-bootstrap.sh'),$linuxWork,'observer-stop')
        $observerResult = Get-Content -LiteralPath (Join-Path $proof 'native-observer.json') -Raw | ConvertFrom-Json
        $receipt.native_observer = $observerResult
        $observerDisarmed = ($observerResult.disarmed -eq $true)
        if (-not $observerDisarmed -or $observerResult.unexpected_mutating_or_unknown_operation -eq $true -or $observerResult.startup_identity_and_state_proven -ne $true -or $observerResult.restart_preservation_required -eq $true) {
          $ownership.violation_observed=$true; Save-OwnedState
          throw 'Owned observer startup/disarm or operation proof uncertain; preserve state'
        }
        try {
          $servedSource = Invoke-QaRequest 'GET' '/source' '' '' $null $null
          if ($servedSource.status -ne 200) { throw 'Actual restarted frontend source endpoint unavailable' }
          $servedSourceHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($servedSource.bytes)).ToLowerInvariant()
          if ($servedSourceHash -ne '135ea616c34a10ed823461712f43d926a6e6dce12fdded96c59bf8830b2c4b1b') { throw 'Actual restarted frontend matching-source differs' }
          $receipt.actual_served_matching_source_sha256 = $servedSourceHash
          $newRegular=Invoke-QaRequest 'GET' '/api/printers' 'native-qa' $nativeSecret $null $null
          $newForbidden=Invoke-QaRequest 'GET' '/admin/users' 'native-qa' $nativeSecret $null $null
          if ($newRegular.status -ne 200 -or $newForbidden.status -ne 403) { throw 'Actual restarted account role proof failed' }
          $receipt.pre_restart_http_account_authentication=$receipt.separate_http_account_authentication
          $receipt.separate_http_account_authentication=[ordered]@{ regular_printers_status=$newRegular.status; regular_admin_status=$newForbidden.status; native_ipp_driver_credential_claim=$false; actual_restarted_process=$true }
        } catch { $ownership.violation_observed=$true; Save-OwnedState; throw 'Actual restarted source or role identity uncertain; preserve fixture' }
        if ($observerResult.capture_complete -ne $true) {
          if ($observerResult.phase -eq 'native') { $ownership.violation_observed=$true; Save-OwnedState }
          throw 'Owned observation is incomplete; no absence/cause/fix inference is authorized'
        }
      } catch {
        if (-not $observerDisarmed) { $ownership.violation_observed=$true; Save-OwnedState }
        throw
      }
    }
  }
  $receipt.native_local_job_count = @(Get-PrintJob -PrinterName $queueName).Count
  if ($receipt.native_local_job_count -ne 0) { $ownership.violation_observed=$true; Save-OwnedState }
  $jobsAfter = Invoke-QaRequest 'GET' '/api/jobs' 'qa-admin' $adminSecret $null $null
  $observedLedger=$null
  if ($jobsAfter.status -eq 200) { $observedLedger = ConvertFrom-Json -InputObject ([Text.Encoding]::UTF8.GetString($jobsAfter.bytes)) -NoEnumerate }
  $receipt.actual_frontend_admin_ledger = [ordered]@{ http_status=$jobsAfter.status; count=$(if ($jobsAfter.status -eq 200) { @($observedLedger).Count } else { $null }); ids=@($observedLedger | ForEach-Object { $_.id }) }
  if ($jobsAfter.status -ne 200 -or $receipt.actual_frontend_admin_ledger.count -ne 0) { $ownership.violation_observed=$true; Save-OwnedState }
  $receipt.actual_receiver_audit = Invoke-RestMethod 'http://127.0.0.1:18631/audit' -NoProxy
  $receipt.receiver_zero_jobs_and_bytes_fields_are_design_invariants_not_independent_measurements = $true
  if ($receipt.native_local_job_count -ne 0 -or $jobsAfter.status -ne 200 -or $receipt.actual_frontend_admin_ledger.count -ne 0 -or $receipt.actual_receiver_audit.refused_operations -ne 0) {
    $ownership.violation_observed=$true
    Save-OwnedState
    throw 'Unexpected job/operation or missing ledger proof; no Print/cancel/replay/retry is authorized'
  }
} finally {
  try {
    if ($ownership.runtime_start_attempted) {
      $ownership.zero_job_teardown_proven = $false
      Save-OwnedState
      $finalNativeCount=$null; $finalLedgerCount=$null; $finalLedgerStatus=$null; $finalRefusedCount=$null
      try {
        $nativeForProof = Get-Printer | Where-Object Name -eq $queueName
        $finalNativeCount = $(if ($null -eq $nativeForProof) { 0 } else { @(Get-PrintJob -PrinterName $queueName).Count })
        $receipt.final_native_job_count = $finalNativeCount
      } catch { $receipt.final_native_job_read_error_type = $_.Exception.GetType().FullName }
      if ($frontendReady -and $adminSecret -and (-not $observerAttempted -or $observerDisarmed)) {
        try {
          $finalLedger = Invoke-QaRequest 'GET' '/api/jobs' 'qa-admin' $adminSecret $null $null
          $finalLedgerStatus = $finalLedger.status
          $receipt.final_ledger_read_status = $finalLedgerStatus
          if ($finalLedgerStatus -eq 200) {
            $finalRows = ConvertFrom-Json -InputObject ([Text.Encoding]::UTF8.GetString($finalLedger.bytes)) -NoEnumerate
            $finalLedgerCount = @($finalRows).Count
            $receipt.final_actual_frontend_ledger = [ordered]@{ count=$finalLedgerCount; ids=@($finalRows | ForEach-Object { $_.id }) }
          }
          $receipt.final_actual_receiver_audit = Invoke-RestMethod 'http://127.0.0.1:18631/audit' -NoProxy
          $finalRefusedCount = $receipt.final_actual_receiver_audit.refused_operations
        } catch { $receipt.final_counter_read_error_type = $_.Exception.GetType().FullName }
      }
      if ($finalNativeCount -gt 0 -or $finalLedgerCount -gt 0 -or $finalRefusedCount -gt 0) { $ownership.violation_observed=$true }
      $ownership.zero_job_teardown_proven = (-not $ownership.violation_observed) -and ($finalNativeCount -eq 0) -and ($finalLedgerStatus -eq 200) -and ($finalLedgerCount -eq 0) -and ($finalRefusedCount -eq 0)
      $receipt.zero_job_teardown_proven = $ownership.zero_job_teardown_proven
      $receipt.runtime_preserved_on_disposable_vm = -not $ownership.zero_job_teardown_proven
      Save-OwnedState
    }
    if ($nativeNameAbsentConfirmed -and (-not $ownership.runtime_start_attempted -or $ownership.zero_job_teardown_proven)) {
      $ownedQueue = Get-Printer -Name $queueName -ErrorAction SilentlyContinue
      if ($null -ne $ownedQueue) {
        $ownJobs = @(Get-PrintJob -PrinterName $queueName)
        if ($ownJobs.Count -eq 0) {
          $ownedPort = $ownedQueue.PortName
          Remove-Printer -Name $queueName
          $receipt.owned_native_queue_removed = $true
          if (($portBaseline -notcontains $ownedPort) -and -not (Get-Printer | Where-Object PortName -eq $ownedPort)) {
            if (Get-PrinterPort -Name $ownedPort -ErrorAction SilentlyContinue) { Remove-PrinterPort -Name $ownedPort }
          }
        } else {
          $receipt.owned_native_queue_removed = $false
          $receipt.unexpected_native_jobs_preserved = $ownJobs.Count
          $ownership.violation_observed=$true; $ownership.zero_job_teardown_proven=$false; Save-OwnedState
          throw 'Unexpected native jobs preserved with fixture state; no teardown or cancellation'
        }
      }
      # Native Add may register the inbox class driver normally; remove only that new registration.
      if ($ownership.inbox_ipp_registered_before -eq $false) {
        if (-not (Get-Printer | Where-Object DriverName -eq 'Microsoft IPP Class Driver')) {
          if (Get-PrinterDriver -Name 'Microsoft IPP Class Driver' -ErrorAction SilentlyContinue) { Remove-PrinterDriver -Name 'Microsoft IPP Class Driver' }
        }
      }
      $driverRegisteredAfter = ($null -ne (Get-PrinterDriver -Name 'Microsoft IPP Class Driver' -ErrorAction SilentlyContinue))
      $receipt.inbox_driver_registration_restored = ($driverRegisteredAfter -eq $ownership.inbox_ipp_registered_before)
    }
    if ($null -ne $printerBaseline -and (-not $ownership.runtime_start_attempted -or $ownership.zero_job_teardown_proven)) {
      $current = @(Get-Printer | Select-Object Name,DriverName,PortName)
      $beforeCanonical = @($printerBaseline | ForEach-Object { $_.Name + "`0" + $_.DriverName + "`0" + $_.PortName } | Sort-Object)
      $afterCanonical = @($current | ForEach-Object { $_.Name + "`0" + $_.DriverName + "`0" + $_.PortName } | Sort-Object)
      $receipt.original_native_printers_preserved = (($beforeCanonical -join "`n") -ceq ($afterCanonical -join "`n"))
      $defaultAfter = @(Get-CimInstance Win32_Printer | Where-Object Default | Select-Object -ExpandProperty Name)
      if (($defaultBefore.Count -eq 1) -and (($defaultBefore -join "`0") -cne ($defaultAfter -join "`0"))) {
        $originalDefault = Get-CimInstance Win32_Printer | Where-Object Name -eq $defaultBefore[0]
        if ($null -ne $originalDefault) {
          $restoreDefault = Invoke-CimMethod -InputObject $originalDefault -MethodName SetDefaultPrinter
          $receipt.owned_auto_default_restoration_status = $restoreDefault.ReturnValue
          $defaultAfter = @(Get-CimInstance Win32_Printer | Where-Object Default | Select-Object -ExpandProperty Name)
        }
      }
      $receipt.native_default_unchanged = (($defaultBefore -join "`0") -ceq ($defaultAfter -join "`0"))
    }
  } catch {
    if ($ownership.runtime_start_attempted) {
      $ownership.zero_job_teardown_proven=$false; $ownership.violation_observed=$true; Save-OwnedState
      $receipt.runtime_preserved_on_disposable_vm=$true
    }
    throw
  } finally {
    try {
      if (-not $ownership.runtime_start_attempted -or ($ownership.zero_job_teardown_proven -and -not $ownership.violation_observed)) {
        if ($ownership.distro_absent_before) {
          $receipt.owned_wsl_unregister_called=$false
          $receipt.owned_wsl_unregister_exit=$null
          $receipt.owned_wsl_before_teardown = Get-FerriprintWslState -DistributionName $distro
          if (-not $receipt.owned_wsl_before_teardown.registration_observed) { throw 'Owned WSL CLI state unknown; preserve fixture rather than infer absence or delete state' }
          if ($receipt.owned_wsl_before_teardown.registered) {
            & wsl.exe --terminate $distro
            $receipt.owned_wsl_unregister_called=$true
            & wsl.exe --unregister $distro
            $receipt.owned_wsl_unregister_exit=$LASTEXITCODE
          }
          $receipt.owned_wsl_after_teardown = Get-FerriprintWslState -DistributionName $distro
          $receipt.owned_wsl_unregistered = $receipt.owned_wsl_after_teardown.registration_observed -and -not $receipt.owned_wsl_after_teardown.registered
          if (($receipt.owned_wsl_unregister_called -and $receipt.owned_wsl_unregister_exit -ne 0) -or -not $receipt.owned_wsl_unregistered) { throw 'Owned WSL CLI cleanup or post-list absence proof failed' }
        }
        $receipt.owned_loopback_listener_ports_closed = (@(Get-NetTCPConnection -State Listen -LocalPort 18631,18634 -ErrorAction SilentlyContinue).Count -eq 0)
        foreach ($ownedScratch in @('build','owned-distro','debian-rootfs.tar.gz','receiver.log','frontend.log')) {
          $path = Join-Path $Work $ownedScratch
          if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
        }
      } else {
        $receipt.runtime_preserved_on_disposable_vm=$true
        $receipt.preservation_scope='No queue/state/distro teardown or job/credential export; provider automatic VM disposal is outside this controller'
      }
    } catch {
      if ($ownership.runtime_start_attempted) {
        $ownership.zero_job_teardown_proven=$false; $ownership.violation_observed=$true; Save-OwnedState
        $receipt.further_teardown_blocked=$true
      }
      throw
    } finally {
      $adminSecret=$null; $nativeSecret=$null; $http.Dispose()
      $receipt.spooler_after = (Get-Service Spooler).Status.ToString()
      $receipt | ConvertTo-Json -Depth 10 -EscapeHandling EscapeNonAscii | Set-Content -LiteralPath (Join-Path $proof 'windows-local-acceptance-receipt.json') -Encoding UTF8
    }
  }
}
if (-not $receipt.original_native_printers_preserved -or -not $receipt.native_default_unchanged -or -not $receipt.inbox_driver_registration_restored -or -not $receipt.owned_loopback_listener_ports_closed) {
  throw 'Owned cleanup did not preserve original native printer/default/inbox-registration state or close the owned listener ports'
}
