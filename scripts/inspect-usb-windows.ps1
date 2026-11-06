param([string]$OutputPath)

# Read-only diagnostic evidence for development on Windows.
# This script never opens a COM port or changes PnP/device settings.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Read-Property([string]$instance, [string]$key) {
    $property = Get-PnpDeviceProperty -InstanceId $instance -KeyName $key -ErrorAction SilentlyContinue
    if ($null -ne $property) { return $property.Data }
    return $null
}

$usbPorts = @()
$otherPorts = @()
$present = @(Get-PnpDevice -PresentOnly | Select-Object -ExpandProperty InstanceId)
foreach ($port in Get-CimInstance Win32_SerialPort | Sort-Object DeviceID) {
    if ($present -notcontains $port.PNPDeviceID) { continue }
    $cursor = [string]$port.PNPDeviceID
    $chain = @()
    $usb = $null
    for ($depth = 0; $depth -lt 8 -and $cursor; $depth++) {
        $chain += $cursor
        # A physical USB device, not an MI_xx composite interface.
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
        $otherPorts += [PSCustomObject]@{ port = $port.DeviceID; name = $port.Name; instance_id = $port.PNPDeviceID }
        continue
    }
    $usbPorts += [PSCustomObject]@{
        port = $port.DeviceID
        status = $port.Status
        name = Read-Property $usb.instance_id 'DEVPKEY_Device_BusReportedDeviceDesc'
        vid = $usb.vid
        pid = $usb.pid
        serial_candidate = $usb.instance_tail
        serial_source = 'USB physical-device instance tail; confirm ID_SERIAL_SHORT on Linux'
        interface_instance = $port.PNPDeviceID
        usb_instance = $usb.instance_id
        interface_service = Read-Property $port.PNPDeviceID 'DEVPKEY_Device_Service'
        interface_compatible_ids = @(Read-Property $port.PNPDeviceID 'DEVPKEY_Device_CompatibleIds')
        interface_problem_code = Read-Property $port.PNPDeviceID 'DEVPKEY_Device_ProblemCode'
        interface_driver_version = Read-Property $port.PNPDeviceID 'DEVPKEY_Device_DriverVersion'
        usb_service = Read-Property $usb.instance_id 'DEVPKEY_Device_Service'
        usb_compatible_ids = @(Read-Property $usb.instance_id 'DEVPKEY_Device_CompatibleIds')
        location_paths = @(Read-Property $usb.instance_id 'DEVPKEY_Device_LocationPaths')
        parent_chain = $chain
    }
}

$evidence = [PSCustomObject]@{
    captured_at = [DateTimeOffset]::Now.ToString('o')
    computer = $env:COMPUTERNAME
    source = 'Win32_SerialPort and present-only Windows PnP properties'
    opened_serial_ports = $false
    usb_serial_ports = $usbPorts
    non_usb_serial_ports = $otherPorts
    limitations = @(
        'No Linux udev/sysfs/termios evidence is available from this Windows inspection.'
        'A composite parent requires a separate interface-safety check before Linux authorization control.'
        'Bootloader identities were not observed.'
    )
}
$json = $evidence | ConvertTo-Json -Depth 8
if ($OutputPath) {
    $path = [IO.Path]::GetFullPath($OutputPath)
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path)) | Out-Null
    [IO.File]::WriteAllText($path, $json, (New-Object System.Text.UTF8Encoding($false)))
}
Write-Output $json
