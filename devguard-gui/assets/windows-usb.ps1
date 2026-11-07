param([switch]$Watch)

# Read-only development diagnostics. Never opens a serial port or writes PnP settings.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$script:deviceProperties = @{}

function Read-Property([string]$instance, [string]$key) {
    if (-not $script:deviceProperties.ContainsKey($instance)) {
        $values = @{}
        $properties = @(Get-PnpDeviceProperty -InstanceId $instance -ErrorAction SilentlyContinue |
            Where-Object { $null -ne $_.PSObject.Properties['Data'] })
        foreach ($property in $properties) { $values[[string]$property.KeyName] = $property.Data }
        $script:deviceProperties[$instance] = $values
    }
    $values = $script:deviceProperties[$instance]
    if ($values.ContainsKey($key)) { return $values[$key] }
    return $null
}

function Read-Snapshot([bool]$automatic) {
    # Fresh per snapshot; one property query per instance, not per field.
    $script:deviceProperties = @{}
    $devices = @()
    $ignored = @()
    $present = @(Get-PnpDevice -PresentOnly -Class Ports | Select-Object -ExpandProperty InstanceId)
    foreach ($port in Get-CimInstance Win32_SerialPort | Sort-Object DeviceID) {
        # Bluetooth virtual COM ports must not inherit USB identity from their radio.
        if ($port.PNPDeviceID -notmatch '^USB\\VID_[0-9A-Fa-f]{4}&PID_[0-9A-Fa-f]{4}') {
            $ignored += [string]$port.DeviceID
            continue
        }
        if ($present -notcontains $port.PNPDeviceID) { continue }
        $cursor = [string]$port.PNPDeviceID
        $chain = @()
        $usb = $null
        for ($depth = 0; $depth -lt 8 -and $cursor; $depth++) {
            $chain += $cursor
            if ($cursor -match '^USB\\VID_([0-9A-Fa-f]{4})&PID_([0-9A-Fa-f]{4})\\([^\\]+)$') {
                $usb = [PSCustomObject]@{
                    vid = $Matches[1].ToLowerInvariant()
                    pid = $Matches[2].ToLowerInvariant()
                    instance_tail = $Matches[3]
                    instance_id = $cursor
                }
                break
            }
            $parent = Read-Property $cursor 'DEVPKEY_Device_Parent'
            if (-not $parent -or $chain -contains [string]$parent) { break }
            $cursor = [string]$parent
        }
        if ($null -eq $usb) {
            $ignored += [string]$port.DeviceID
            continue
        }
        $name = Read-Property $usb.instance_id 'DEVPKEY_Device_BusReportedDeviceDesc'
        if (-not $name) { $name = $port.Name }
        # '&' in the instance tail indicates a generated location identity,
        # not a trustworthy USB serial-number candidate.
        $serial = $null
        if ($usb.instance_tail -notmatch '&') { $serial = $usb.instance_tail }
        $service = Read-Property $port.PNPDeviceID 'DEVPKEY_Device_Service'
        $driver = Read-Property $port.PNPDeviceID 'DEVPKEY_Device_DriverVersion'
        $problem = Read-Property $port.PNPDeviceID 'DEVPKEY_Device_ProblemCode'
        $usbService = Read-Property $usb.instance_id 'DEVPKEY_Device_Service'
        $devices += [PSCustomObject]@{
            instance_id = [string]$port.PNPDeviceID
            usb_instance_id = [string]$usb.instance_id
            port = [string]$port.DeviceID
            name = [string]$name
            status = [string]$port.Status
            vid = [string]$usb.vid
            pid = [string]$usb.pid
            serial_candidate = $serial
            driver_service = $(if ($service) { [string]$service } else { $null })
            driver_version = $(if ($driver) { [string]$driver } else { $null })
            problem_code = $(if ($null -ne $problem) { [uint32]$problem } else { $null })
            composite_parent = ($usbService -eq 'usbccgp')
            location_paths = @(Read-Property $usb.instance_id 'DEVPKEY_Device_LocationPaths')
        }
    }
    return [PSCustomObject]@{
        captured_at = [DateTimeOffset]::Now.ToString('o')
        computer = $env:COMPUTERNAME
        automatic_updates = $automatic
        devices = $devices
        ignored_non_usb_ports = $ignored
    }
}

function Publish($message) {
    # One complete JSON line per event, flushed by WriteLine.
    [Console]::Out.WriteLine(($message | ConvertTo-Json -Depth 8 -Compress))
    [Console]::Out.Flush()
}

$source = 'devguard-usb-' + [Guid]::NewGuid().ToString('N')
$subscription = $null
$watchError = $null
try {
    if ($Watch) {
        try {
            # Subscribe before enumerating to avoid losing a change during startup.
            # Without -Action, Register-WmiEvent can return no object even
            # though the subscription is live. Inspect the registered listener.
            Register-WmiEvent -Class Win32_DeviceChangeEvent -SourceIdentifier $source | Out-Null
            $subscription = Get-EventSubscriber -SourceIdentifier $source -ErrorAction Stop
            if ($null -eq $subscription) { throw 'Device-change subscription was not registered' }
        } catch {
            $watchError = $_.Exception.Message
        }
    }
    $automatic = ($null -ne $subscription)
    Publish @{ event = 'snapshot'; snapshot = (Read-Snapshot $automatic) }
    if ($watchError) {
        Publish @{ event = 'watch_warning'; message = "Automatic device updates are unavailable: $watchError. Use Refresh to rescan." }
    }
    while ($automatic) {
        $change = Wait-Event -SourceIdentifier $source
        Remove-Event -EventIdentifier $change.EventIdentifier
        # Only WMI notifications trigger a new enumeration. No polling timer.
        Publish @{ event = 'snapshot'; snapshot = (Read-Snapshot $true) }
    }
} catch {
    Publish @{ event = 'error'; message = "Could not inspect USB serial devices: $($_.Exception.Message). Use Refresh to retry." }
    exit 1
} finally {
    if ($null -ne $subscription) {
        Unregister-Event -SourceIdentifier $source -ErrorAction SilentlyContinue
        Remove-Event -SourceIdentifier $source -ErrorAction SilentlyContinue
    }
}
