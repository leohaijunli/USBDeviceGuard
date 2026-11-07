#![cfg(target_os = "windows")]

//! Test the scanner's external PowerShell interface with an isolated OS fixture.

use std::os::windows::process::CommandExt;
use std::process::Command;

use base64::{engine::general_purpose::STANDARD, Engine};

#[test]
fn usb_serial_inventory_excludes_bluetooth_ports_with_usb_ancestors() {
    let fixture = r#"
function Get-CimInstance {
    [CmdletBinding()] param([string]$ClassName)
    @(
        [PSCustomObject]@{ DeviceID='COM4'; Name='USB Serial Device (COM4)'; Status='OK'; PNPDeviceID='USB\VID_1209&PID_0001&MI_00\IFACE' }
        [PSCustomObject]@{ DeviceID='COM5'; Name='Bluetooth serial (COM5)'; Status='OK'; PNPDeviceID='BTHENUM\SERVICE\BT-PORT' }
    )
}

function Get-PnpDevice {
    [CmdletBinding()] param([switch]$PresentOnly, [string[]]$Class)
    @(
        [PSCustomObject]@{ InstanceId='USB\VID_1209&PID_0001&MI_00\IFACE' }
        [PSCustomObject]@{ InstanceId='BTHENUM\SERVICE\BT-PORT' }
    )
}
function Get-PnpDeviceProperty {
    [CmdletBinding()] param([string]$InstanceId, [string[]]$KeyName)
    $values = @{}
    switch ($InstanceId) {
        'USB\VID_1209&PID_0001&MI_00\IFACE' {
            $values = @{ DEVPKEY_Device_Parent='USB\VID_1209&PID_0001\V6X-RUST-001'; DEVPKEY_Device_Service='usbser'; DEVPKEY_Device_DriverVersion='1.2.3'; DEVPKEY_Device_ProblemCode=0 }
        }
        'USB\VID_1209&PID_0001\V6X-RUST-001' {
            $values = @{ DEVPKEY_Device_BusReportedDeviceDesc='V6X Rust CDC'; DEVPKEY_Device_Service='usbccgp'; DEVPKEY_Device_LocationPaths=@('usb-port-3') }
        }
        'BTHENUM\SERVICE\BT-PORT' { $values = @{ DEVPKEY_Device_Parent='USB\VID_8087&PID_0033\BLUETOOTH'; DEVPKEY_Device_Service='BTHMODEM'; DEVPKEY_Device_ProblemCode=0 } }
        'USB\VID_8087&PID_0033\BLUETOOTH' { $values = @{ DEVPKEY_Device_BusReportedDeviceDesc='USB Bluetooth adapter'; DEVPKEY_Device_Service='bthusb'; DEVPKEY_Device_LocationPaths=@('usb-port-10') } }
    }
    $keys = if ($KeyName) { $KeyName } else { @($values.Keys) }
    foreach ($key in $keys) {
        if ($values.ContainsKey($key)) { [PSCustomObject]@{ KeyName=$key; Data=$values[$key] } }
    }
}
"#;
    let script = format!(
        "{fixture}\n& {{\n{}\n}}",
        include_str!("../assets/windows-usb.ps1")
    );
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &STANDARD.encode(bytes),
        ])
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let event: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let devices = event["snapshot"]["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["port"], "COM4");
    assert_eq!(devices[0]["serial_candidate"], "V6X-RUST-001");
    assert_eq!(
        event["snapshot"]["ignored_non_usb_ports"],
        serde_json::json!(["COM5"])
    );
}

#[test]
fn successful_wmi_subscription_without_a_return_value_still_enters_the_watch_loop() {
    // All OS calls are stubbed. Registration deliberately returns no object.
    let fixture = r#"
$script:registered = $false
function Register-WmiEvent {
    [CmdletBinding()] param([string]$Class, [string]$SourceIdentifier)
    $script:registered = $true
}
function Get-EventSubscriber {
    [CmdletBinding()] param([string]$SourceIdentifier)
    if ($script:registered) { [PSCustomObject]@{ SourceIdentifier=$SourceIdentifier } }
}
function Get-PnpDevice {
    [CmdletBinding()] param([switch]$PresentOnly, [string[]]$Class)
    if (-not $script:registered) { throw 'Enumeration started before subscription' }
}
function Get-CimInstance {
    [CmdletBinding()] param([string]$ClassName)
    if (-not $script:registered) { throw 'Enumeration started before subscription' }
}
function Wait-Event {
    [CmdletBinding()] param([string]$SourceIdentifier)
    throw 'ISOLATED-WATCH-LOOP-REACHED'
}
function Unregister-Event {
    [CmdletBinding()] param([string]$SourceIdentifier)
    $script:registered = $false
    [Console]::Error.WriteLine('ISOLATED-SUBSCRIPTION-CLEANED')
}
function Remove-Event {
    [CmdletBinding()] param([string]$SourceIdentifier, [int]$EventIdentifier)
}
"#;
    let script = format!(
        "{fixture}\n& {{\n{}\n}} -Watch",
        include_str!("../assets/windows-usb.ps1")
    );
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &STANDARD.encode(bytes),
        ])
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0]["event"], "snapshot");
    assert_eq!(events[0]["snapshot"]["automatic_updates"], true);
    assert_eq!(events[1]["event"], "error");
    assert!(events[1]["message"]
        .as_str()
        .unwrap()
        .contains("ISOLATED-WATCH-LOOP-REACHED"));
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ISOLATED-SUBSCRIPTION-CLEANED"));
}
