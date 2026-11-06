param(
    [ValidateSet('Check', 'Test', 'Lint', 'Native', 'Run')]
    [string]$Step = 'Test'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$OutputEncoding = New-Object System.Text.UTF8Encoding($false)

$source = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$mirror = Join-Path $env:LOCALAPPDATA 'Temp\opencode\devguard-nsh-validation'
$marker = Join-Path $mirror '.devguard-validation-root'
$markerText = 'devguard-owned-validation-copy-v1'

# NSH 0.4.0's JS test bundler cannot resolve Windows extended UNC paths.
# Build an owned local snapshot; the canonical project stays at $source.
if (Test-Path -LiteralPath $mirror) {
    if (-not (Test-Path -LiteralPath $marker)) {
        throw "Refusing to overwrite an unmarked directory: $mirror"
    }
    if ((Get-Content -LiteralPath $marker -Raw).Trim() -ne $markerText) {
        throw "Invalid validation-directory marker: $mirror"
    }
} else {
    New-Item -ItemType Directory -Path $mirror | Out-Null
    [IO.File]::WriteAllText($marker, $markerText, $OutputEncoding)
}

$manifest = @()
function Copy-SourceFile([string]$relative) {
    $from = Join-Path $source $relative
    if (-not (Test-Path -LiteralPath $from -PathType Leaf)) { return }
    $to = Join-Path $mirror $relative
    New-Item -ItemType Directory -Force -Path (Split-Path $to -Parent) | Out-Null
    Copy-Item -LiteralPath $from -Destination $to -Force
    $originalHash = (Get-FileHash -LiteralPath $from -Algorithm SHA256).Hash
    $copiedHash = (Get-FileHash -LiteralPath $to -Algorithm SHA256).Hash
    if ($originalHash -ne $copiedHash) { throw "Source changed during copy: $relative" }
    $script:manifest += [PSCustomObject]@{ path = $relative; sha256 = $originalHash }
}

Copy-SourceFile 'Cargo.toml'
Copy-SourceFile 'Cargo.lock'
foreach ($crate in @('devguard-core', 'devguardd', 'devguardctl', 'devguard-gui')) {
    Copy-SourceFile "$crate\Cargo.toml"
    foreach ($directory in @('src', 'tests', 'assets')) {
        $from = Join-Path $source "$crate\$directory"
        $to = Join-Path $mirror "$crate\$directory"
        # Only source subdirectories inside our marked validation copy are removed.
        if (Test-Path -LiteralPath $to) { Remove-Item -LiteralPath $to -Recurse -Force }
        if (Test-Path -LiteralPath $from) {
            foreach ($file in Get-ChildItem -LiteralPath $from -Recurse -File) {
                Copy-SourceFile $file.FullName.Substring($source.Length + 1)
            }
        }
    }
}
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $source 'devguard-gui') -File) {
    if ($file.Name -match '\.(json|tsx)$') { Copy-SourceFile "devguard-gui\$($file.Name)" }
}
$evidence = Join-Path $mirror 'artifacts'
New-Item -ItemType Directory -Force -Path $evidence | Out-Null
$manifestJson = $manifest | ConvertTo-Json -Depth 3
[IO.File]::WriteAllText((Join-Path $evidence 'source-sha256.json'), $manifestJson, $OutputEncoding)

$nsh = Join-Path $env:LOCALAPPDATA 'nsh\bin\nsh.exe'
if (-not (Test-Path -LiteralPath $nsh)) {
    $nsh = (Get-Command nsh -ErrorAction Stop).Source
}
$version = & $nsh --version
if ($LASTEXITCODE -ne 0 -or $version -notmatch '^nsh 0\.4\.0\b') {
    throw 'This spike requires the NSH 0.4.0 CLI matching its Rust crates.'
}
Write-Output "$version | validation snapshot: $mirror"
# The validation copy is a different source path from the canonical workspace;
# sharing one target dir corrupts fingerprints and link arguments.
$env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'Temp\opencode\devguard-target-mirror'

Push-Location (Join-Path $mirror 'devguard-gui')
try {
    switch ($Step) {
        'Check' { & $nsh check }
        'Test' { & $nsh test --junit (Join-Path $evidence 'headless.xml') }
        'Lint' {
            cargo fmt -p devguard-gui -- --check
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            cargo clippy -p devguard-gui --all-targets -- -D warnings
        }
        'Native' {
            $steps = Join-Path $PSScriptRoot 'nsh-native.steps.js'
            # Windows PowerShell's native pipeline can replace CJK with '?'.
            # Supply UTF-8 explicitly through the process's standard input.
            $start = New-Object System.Diagnostics.ProcessStartInfo
            $start.FileName = $nsh
            $start.Arguments = 'test nsh.json --target windows'
            $start.WorkingDirectory = (Get-Location).ProviderPath
            $start.UseShellExecute = $false
            $start.RedirectStandardInput = $true
            $process = New-Object System.Diagnostics.Process
            $process.StartInfo = $start
            try {
                if (-not $process.Start()) { throw 'Could not start native NSH verification.' }
                $bytes = [IO.File]::ReadAllBytes($steps)
                $process.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
                $process.StandardInput.BaseStream.Flush()
                $process.StandardInput.Close()
                $process.WaitForExit()
                $LASTEXITCODE = $process.ExitCode
            } finally {
                $process.Dispose()
            }
        }
        'Run' { cargo run -p devguard-gui }
    }
    $code = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $code
