# SPDX-License-Identifier: AGPL-3.0-only
# Native Windows .NET Framework PrintCapabilities/PrintTicket only; no document/job API.
# Also records the exact Get-Printer-Attributes evidence the driver negotiated.
param(
  [Parameter(Mandatory=$true)][string]$QueueName,
  [Parameter(Mandatory=$true)][string]$ProofDirectory
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName ReachFramework
Add-Type -AssemblyName System.Printing
$server = New-Object System.Printing.LocalPrintServer
$queue = $server.GetPrintQueue($QueueName)
try {
  $capabilities = $queue.GetPrintCapabilities()
  $capabilityXml = $queue.GetPrintCapabilitiesAsXml()
  # Read the stream once, before it is copied and disposed: MemoryStream has no
  # XML accessors, so the feature names come from a parsed copy of the same bytes.
  $capabilityBytes = $capabilityXml.ToArray()
  $destination = [IO.File]::Create((Join-Path $ProofDirectory 'native-print-capabilities.xml'))
  try { $destination.Write($capabilityBytes, 0, $capabilityBytes.Length) } finally { $destination.Dispose(); $capabilityXml.Dispose() }
  $capabilityDocument = New-Object Xml.XmlDocument
  $capabilityDocument.PreserveWhitespace = $false
  $capabilityDocument.LoadXml([Text.Encoding]::UTF8.GetString($capabilityBytes))
  $base = $queue.DefaultPrintTicket
  $baseStream = [IO.File]::Create((Join-Path $ProofDirectory 'native-default-print-ticket.xml'))
  try { $base.SaveTo($baseStream) } finally { $baseStream.Dispose() }
  $specs = @(
    @{ key='a3-borderless-high'; media=[System.Printing.PageMediaSizeName]::ISOA3; quality=[System.Printing.OutputQuality]::High; duplex=[System.Printing.Duplexing]::OneSided; borderless=$true },
    @{ key='a4-normal'; media=[System.Printing.PageMediaSizeName]::ISOA4; quality=[System.Printing.OutputQuality]::Normal; duplex=[System.Printing.Duplexing]::OneSided; borderless=$false },
    @{ key='a4-duplex'; media=[System.Printing.PageMediaSizeName]::ISOA4; quality=[System.Printing.OutputQuality]::Normal; duplex=[System.Printing.Duplexing]::TwoSidedLongEdge; borderless=$false }
  )
  $results = @()
  $validatedXml = @{}
  foreach ($spec in $specs) {
    $requiredWidthMm = $(if ($spec.key -eq 'a3-borderless-high') { 297.0 } else { 210.0 })
    $requiredHeightMm = $(if ($spec.key -eq 'a3-borderless-high') { 420.0 } else { 297.0 })
    $media = @($capabilities.PageMediaSizeCapability | Where-Object {
      $_.PageMediaSizeName -eq $spec.media -and $null -ne $_.Width -and $null -ne $_.Height -and
        [Math]::Abs($_.Width * 25.4 / 96.0 - $requiredWidthMm) -le 0.1 -and
        [Math]::Abs($_.Height * 25.4 / 96.0 - $requiredHeightMm) -le 0.1
    } | Sort-Object @{Expression={ [Math]::Abs($_.Width * 25.4 / 96.0 - $requiredWidthMm) + [Math]::Abs($_.Height * 25.4 / 96.0 - $requiredHeightMm) }})
    if ($media.Count -eq 0) {
      $results += [ordered]@{ recipe=$spec.key; native_media_available=$false; matching_media_count=$media.Count }
      continue
    }
    $ticket = New-Object System.Printing.PrintTicket
    $ticket.PageMediaSize = $media[0]
    $ticket.OutputQuality = $spec.quality
    $ticket.Duplexing = $spec.duplex
    $ticket.PageOrientation = [System.Printing.PageOrientation]::Portrait
    $ticket.CopyCount = 1
    # .NET's typed PageResolutionCapability only materializes options it can map
    # onto a known Print Schema keyword. The IPP class driver names its option in
    # the ns0001 (IPP keywords) namespace, so the typed collection stays empty even
    # though the driver's own PrintCapabilities document advertises 300x300 under
    # psk:PageResolution. Read the exposure from the driver's document, and keep
    # using the validated ticket for what was actually applied.
    $resolutionXml = @($capabilityDocument.GetElementsByTagName('psf:Option') | Where-Object {
      $_.ParentNode.GetAttribute('name') -eq 'psk:PageResolution'
    } | Where-Object {
      # ResolutionX/Y are ScoredProperty *names*, not element names.
      $x = @($_.GetElementsByTagName('psf:ScoredProperty') | Where-Object { $_.GetAttribute('name') -eq 'psk:ResolutionX' } | ForEach-Object { $_.InnerText })
      $y = @($_.GetElementsByTagName('psf:ScoredProperty') | Where-Object { $_.GetAttribute('name') -eq 'psk:ResolutionY' } | ForEach-Object { $_.InnerText })
      $x.Count -eq 1 -and $y.Count -eq 1 -and $x[0] -eq '300' -and $y[0] -eq '300'
    })
    $resolution = @($capabilities.PageResolutionCapability | Where-Object { $_.X -eq 300 -and $_.Y -eq 300 })
    if ($resolution.Count -gt 0) { $ticket.PageResolution = $resolution[0] }
    $borderlessAvailable = $capabilities.PageBorderlessCapability -contains [System.Printing.PageBorderless]::Borderless
    if ($spec.borderless -and $borderlessAvailable) { $ticket.PageBorderless = [System.Printing.PageBorderless]::Borderless }
    if (-not $spec.borderless -and ($capabilities.PageBorderlessCapability -contains [System.Printing.PageBorderless]::None)) {
      $ticket.PageBorderless = [System.Printing.PageBorderless]::None
    }
    $validation = $queue.MergeAndValidatePrintTicket($base, $ticket)
    $actual = $validation.ValidatedPrintTicket
    $ticketPath = Join-Path $ProofDirectory ($spec.key + '-native-validated-ticket.xml')
    $ticketStream = [IO.File]::Create($ticketPath)
    try { $actual.SaveTo($ticketStream) } finally { $ticketStream.Dispose() }
    # Parse the saved ticket so checks read what the driver recorded, not only what
    # the typed properties surface.
    $ticketDocument = New-Object Xml.XmlDocument
    $ticketDocument.PreserveWhitespace = $false
    $ticketDocument.LoadXml([IO.File]::ReadAllText($ticketPath))
    $validatedXml[$spec.key] = $ticketDocument
    $actualCaps = $queue.GetPrintCapabilities($actual)
    $results += [ordered]@{
      recipe = $spec.key
      native_media_available = $true
      requested_borderless = $spec.borderless
      native_borderless_capability = $borderlessAvailable
      requested_quality_exposed = ($capabilities.OutputQualityCapability -contains $spec.quality)
      requested_duplex_exposed = ($capabilities.DuplexingCapability -contains $spec.duplex)
      requested_resolution_exposed = ($resolutionXml.Count -gt 0)
      conditional_quality_exposed = ($actualCaps.OutputQualityCapability -contains $spec.quality)
      conditional_duplex_exposed = ($actualCaps.DuplexingCapability -contains $spec.duplex)
      conditional_borderless_exposed = ($actualCaps.PageBorderlessCapability -contains [System.Printing.PageBorderless]::Borderless)
      validation_conflict = $validation.ConflictStatus.ToString()
      actual_media = $(if ($null -ne $actual.PageMediaSize.PageMediaSizeName) { $actual.PageMediaSize.PageMediaSizeName.ToString() } else { $null })
      actual_width_dip = $actual.PageMediaSize.Width
      actual_height_dip = $actual.PageMediaSize.Height
      actual_quality = $(if ($null -ne $actual.OutputQuality) { $actual.OutputQuality.ToString() } else { $null })
      actual_duplex = $(if ($null -ne $actual.Duplexing) { $actual.Duplexing.ToString() } else { $null })
      actual_borderless = $(if ($null -ne $actual.PageBorderless) { $actual.PageBorderless.ToString() } else { $null })
      actual_copies = $actual.CopyCount
      actual_orientation = $(if ($null -ne $actual.PageOrientation) { $actual.PageOrientation.ToString() } else { $null })
      actual_resolution_x = $actual.PageResolution.X
      actual_resolution_y = $actual.PageResolution.Y
      actual_imageable_area = $(if ($null -ne $actualCaps.PageImageableArea) {
        [ordered]@{ origin_x_dip=$actualCaps.PageImageableArea.OriginWidth; origin_y_dip=$actualCaps.PageImageableArea.OriginHeight; width_dip=$actualCaps.PageImageableArea.ExtentWidth; height_dip=$actualCaps.PageImageableArea.ExtentHeight }
      } else { $null })
    }
  }
  foreach ($result in $results) {
    if (-not $result.native_media_available) {
      $result.required_native_settings_pass = $false
      continue
    }
    $expectedWidthMm = $(if ($result.recipe -eq 'a3-borderless-high') { 297.0 } else { 210.0 })
    $expectedHeightMm = $(if ($result.recipe -eq 'a3-borderless-high') { 420.0 } else { 297.0 })
    $result.actual_width_mm = $(if ($null -ne $result.actual_width_dip) { $result.actual_width_dip * 25.4 / 96.0 } else { $null })
    $result.actual_height_mm = $(if ($null -ne $result.actual_height_dip) { $result.actual_height_dip * 25.4 / 96.0 } else { $null })
    $dimensionsMatch = ($null -ne $result.actual_width_mm) -and ($null -ne $result.actual_height_mm) -and
      ([Math]::Abs($result.actual_width_mm - $expectedWidthMm) -le 0.1) -and ([Math]::Abs($result.actual_height_mm - $expectedHeightMm) -le 0.1)
    $qualityMatch = $result.requested_quality_exposed -and $result.conditional_quality_exposed -and ($result.actual_quality -eq $(if ($result.recipe -eq 'a3-borderless-high') { 'High' } else { 'Normal' }))
    $duplexMatch = $result.requested_duplex_exposed -and $result.conditional_duplex_exposed -and ($result.actual_duplex -eq $(if ($result.recipe -eq 'a4-duplex') { 'TwoSidedLongEdge' } else { 'OneSided' }))
    # The typed ticket property can stay null when the driver's option is named in
    # the IPP keyword namespace; the validated ticket XML is the authority for what
    # the driver actually recorded.
    $ticketResolutionX = @($validatedXml[$result.recipe].GetElementsByTagName('psf:ScoredProperty') | Where-Object { $_.GetAttribute('name') -eq 'psk:ResolutionX' } | ForEach-Object { $_.InnerText })
    $ticketResolutionY = @($validatedXml[$result.recipe].GetElementsByTagName('psf:ScoredProperty') | Where-Object { $_.GetAttribute('name') -eq 'psk:ResolutionY' } | ForEach-Object { $_.InnerText })
    $resolutionMatch = $result.requested_resolution_exposed -and
      (($result.actual_resolution_x -eq 300 -and $result.actual_resolution_y -eq 300) -or
       ($ticketResolutionX.Count -eq 1 -and $ticketResolutionY.Count -eq 1 -and $ticketResolutionX[0] -eq '300' -and $ticketResolutionY[0] -eq '300'))
    $borderlessMatch = $true
    if ($result.requested_borderless) {
      $area = $result.actual_imageable_area
      $borderlessMatch = $result.native_borderless_capability -and $result.conditional_borderless_exposed -and ($result.actual_borderless -eq 'Borderless') -and ($null -ne $area) -and
        ([Math]::Abs($area.origin_x_dip) -le 0.05) -and ([Math]::Abs($area.origin_y_dip) -le 0.05) -and
        ([Math]::Abs($area.width_dip - $result.actual_width_dip) -le 0.5) -and ([Math]::Abs($area.height_dip - $result.actual_height_dip) -le 0.5)
    } else {
      $area = $result.actual_imageable_area
      $borderlessMatch = ($result.actual_borderless -ne 'Borderless') -and ($null -ne $area) -and
        ($area.origin_x_dip -ge 0.5) -and ($area.origin_y_dip -ge 0.5) -and
        (($result.actual_width_dip - $area.origin_x_dip - $area.width_dip) -ge 0.5) -and (($result.actual_height_dip - $area.origin_y_dip - $area.height_dip) -ge 0.5)
    }
    $result.semantic_checks = [ordered]@{ dimensions_match=$dimensionsMatch; quality_match=$qualityMatch; duplex_match=$duplexMatch; resolution_300dpi_match=$resolutionMatch; requested_borderless_or_bordered_imageable_margins_match=$borderlessMatch }
    $result.required_native_settings_pass = $dimensionsMatch -and $qualityMatch -and $duplexMatch -and $resolutionMatch -and $borderlessMatch -and ($result.actual_copies -eq 1) -and ($result.actual_orientation -eq 'Portrait')
  }
  $requiredPassed = ($results.Count -eq 3) -and (@($results | Where-Object { -not $_.required_native_settings_pass }).Count -eq 0)
  [ordered]@{
    native_api = 'System.Printing.LocalPrintServer/GetPrintCapabilities/MergeAndValidatePrintTicket on the actual Windows IPP queue'
    recipes_are_requested_test_templates_not_claimed_imported_site_presets = $true
    native_capability_media_names = @($capabilities.PageMediaSizeCapability | ForEach-Object { if ($null -ne $_.PageMediaSizeName) { $_.PageMediaSizeName.ToString() } else { $null } })
    native_media_width_dip = @($capabilities.PageMediaSizeCapability | ForEach-Object { [ordered]@{ name=$(if ($null -ne $_.PageMediaSizeName) { $_.PageMediaSizeName.ToString() } else { $null }); width=$_.Width; height=$_.Height } })
    native_quality_names = @($capabilities.OutputQualityCapability | ForEach-Object { $_.ToString() })
    native_duplex_names = @($capabilities.DuplexingCapability | ForEach-Object { $_.ToString() })
    native_borderless_names = @($capabilities.PageBorderlessCapability | ForEach-Object { $_.ToString() })
    native_resolution_names = @($capabilities.PageResolutionCapability | ForEach-Object { $_.ToString() })
    native_resolution_xml_options = @($resolutionXml | ForEach-Object { $_.GetAttribute('name') })
    native_imageable_size = $(if ($null -ne $capabilities.PageImageableArea) {
      [ordered]@{ origin_x_dip=$capabilities.PageImageableArea.OriginWidth; origin_y_dip=$capabilities.PageImageableArea.OriginHeight; width_dip=$capabilities.PageImageableArea.ExtentWidth; height_dip=$capabilities.PageImageableArea.ExtentHeight }
    } else { $null })
    native_feature_names = @($capabilityDocument.GetElementsByTagName('psf:Feature') | ForEach-Object { $_.GetAttribute('name') })
    requested_and_actual_tickets = $results
    required_native_settings_pass = $requiredPassed
    imageable_area_is_native_API_metadata_not_physical_paper_measurement = $true
    document_or_job_api_calls = 0
  } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $ProofDirectory 'native-print-ticket-results.json') -Encoding UTF8
  if (-not $requiredPassed) { throw 'Native merged capabilities/tickets did not preserve all required A3/borderless/High and A4/duplex semantics; retain the exact unsupported/conflict evidence, without fallback or Print.' }
} finally {
  $queue.Dispose()
  $server.Dispose()
}
