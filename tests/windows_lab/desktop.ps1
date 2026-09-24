# Disposable WinForms destination. Only this fixture reads back its own
# synthetic text; production credential-fill never reads control values.
param([string]$Binary = "$PSScriptRoot/../../target/debug/agentenv.exe", [string]$Probe = "$PSScriptRoot/../../target/debug/test-probe.exe")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
$Binary = (Resolve-Path $Binary).Path
$Probe = (Resolve-Path $Probe).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ('agentenv-desktop-' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($root) | Out-Null
$config = Join-Path $root 'config.toml'
$marker = Join-Path $root 'resolving'
$synthetic = '  Synthetic 密码🔑 "quoted" \\  '
$form = [Windows.Forms.Form]::new()
$form.Text = 'agentenv disposable desktop fixture'
$form.Width = 560
$form.Height = 260
$form.StartPosition = 'CenterScreen'
$first = [Windows.Forms.TextBox]::new()
$first.SetBounds(20, 20, 500, 30)
$second = [Windows.Forms.TextBox]::new()
$second.SetBounds(20, 70, 500, 30)
$button = [Windows.Forms.Button]::new()
$button.Text = 'Not a text input'
$button.SetBounds(20, 120, 200, 30)
$form.Controls.AddRange(@($first, $second, $button))
$form.Show()

function Set-FixtureConfig([bool]$Delayed) {
    if ($Delayed) {
        $argv = @($Probe, '--resolver-fixture', 'delayed', $synthetic, $marker) | ForEach-Object { ConvertTo-Json -Compress -InputObject $_ }
        $provider = 'provider = "command"' + "`nargv = [" + ($argv -join ', ') + ']'
    } else {
        $provider = 'provider = "env"' + "`nname = " + '"AGENTENV_DESKTOP_FIXTURE"'
    }
    [IO.File]::WriteAllText($config, "version = 1`n[credentials.fixture]`ndescription = `"Synthetic fixture`"`ninject_as = `"FIXTURE`"`n$provider`n", [Text.UTF8Encoding]::new($false))
}
function Run-Fill([Windows.Forms.Control]$Control, [int]$Expected, [bool]$ChangeFocus = $false, [int]$ExpectedPid = $PID) {
    $first.Text = ''; $second.Text = ''
    $form.Activate()
    $Control.Focus() | Out-Null
    [Windows.Forms.Application]::DoEvents()
    $start = [Diagnostics.ProcessStartInfo]::new($Binary)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.WorkingDirectory = $root
    $start.Environment['AGENTENV_FILE'] = $config
    $start.Environment['AGENTENV_NO_PROJECT'] = '1'
    $start.Environment['AGENTENV_DESKTOP_FIXTURE'] = $synthetic
    foreach ($arg in @('--json', 'credential', 'fill', 'fixture', '--backend', 'desktop', '--expect-pid', "$ExpectedPid", '--timeout-ms', '10000')) { $start.ArgumentList.Add($arg) }
    $process = [Diagnostics.Process]::Start($start)
    $process.StandardInput.Close()
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $changed = $false
    try {
        while (-not $process.HasExited) {
            [Windows.Forms.Application]::DoEvents()
            if ($ChangeFocus -and -not $changed -and [IO.File]::Exists($marker)) {
                $second.Focus() | Out-Null
                $changed = $true
            }
            if ($clock.Elapsed.TotalSeconds -gt 20) { throw 'Desktop fixture exceeded its deadline' }
            [Threading.Thread]::Sleep(10)
        }
        $process.WaitForExit()
        # Drain the fixture's message queue after SendInput reports delivery.
        for ($i = 0; $i -lt 20; $i++) { [Windows.Forms.Application]::DoEvents(); [Threading.Thread]::Sleep(10) }
        $stdout = $out.GetAwaiter().GetResult()
        $stderr = $err.GetAwaiter().GetResult()
        if ($stdout.Contains($synthetic) -or $stderr.Contains($synthetic)) { throw 'Synthetic value leaked into CLI output' }
        if ($process.ExitCode -ne $Expected) { throw "Expected exit $Expected; got $($process.ExitCode): $stderr" }
        if ($Expected -eq 0) {
            $result = $stdout | ConvertFrom-Json
            if ($result.backend -ne 'desktop' -or $result.effect -ne 'input-sent') { throw 'Incorrect delivery result' }
            if ($first.Text -cne $synthetic) { throw 'The fixture did not receive the exact Unicode text' }
        } else {
            if ($stdout -ne '' -or $first.Text -ne '' -or $second.Text -ne '') { throw 'A refused operation changed a fixture input' }
        }
        if ($ChangeFocus -and -not $changed) { throw 'The delayed provider did not reach the focus-change gate' }
    } finally {
        if (-not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
        $process.Dispose()
    }
}
try {
    Set-FixtureConfig $false
    Run-Fill $first 0
    $first.UseSystemPasswordChar = $true
    Run-Fill $first 0
    $first.UseSystemPasswordChar = $false
    $first.ReadOnly = $true
    Run-Fill $first 8
    $first.ReadOnly = $false
    Run-Fill $button 8
    Run-Fill $first 8 $false 1
    Set-FixtureConfig $true
    Run-Fill $first 8 $true
    Write-Output 'PASS: Unicode, password Edit, read-only, wrong control, wrong PID, and focus change during resolution (6 cases)'
} finally {
    $form.Close(); $form.Dispose()
    # Only the uniquely created, exact fixture directory is removed.
    [IO.Directory]::Delete($root, $true)
}
