$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

$result = @{
    pagefiles = @()
    pagefile_error = $null
    crash = $null
    crash_error = $null
}

try {
    $result.pagefiles = @(Get-CimInstance -ClassName Win32_PageFileUsage | ForEach-Object { $_.Name })
} catch {
    $result.pagefile_error = $_.Exception.Message
}

try {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\CrashControl')
    if ($null -eq $key) { throw 'CrashControl configuration is unavailable.' }
    try {
        $raw = [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
        $result.crash = @{
            mode = $key.GetValue('CrashDumpEnabled')
            dedicated = $key.GetValue('DedicatedDumpFile', $null, $raw)
            dump_file = $key.GetValue('DumpFile', $null, $raw)
            minidump_dir = $key.GetValue('MinidumpDir', $null, $raw)
        }
    } finally {
        $key.Dispose()
    }
} catch {
    $result.crash_error = $_.Exception.Message
}

ConvertTo-Json -InputObject $result -Depth 4 -Compress
