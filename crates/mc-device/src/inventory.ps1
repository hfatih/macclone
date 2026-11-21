$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Import-Module (Join-Path $PSHOME 'Modules/Storage/Storage.psd1')
$rows = @(Storage\Get-Disk | ForEach-Object {
  [pscustomobject]@{
    number=$_.Number; model=[string]$_.FriendlyName; serial=$_.SerialNumber; id=$_.UniqueId
    bytes=$_.Size; logical=$_.LogicalSectorSize; physical=$_.PhysicalSectorSize; bus=[string]$_.BusType
    boot=$_.IsBoot; system=$_.IsSystem; offline=$_.IsOffline; readonly=$_.IsReadOnly
    clustered=$_.IsClustered; health=[string]$_.HealthStatus
  }
})
ConvertTo-Json -InputObject $rows -Depth 3 -Compress
