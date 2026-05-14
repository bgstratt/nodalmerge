[CmdletBinding()]
param(
    [int]$WarmupIterations = 5,
    [int]$MeasureIterations = 50,
    [int]$ReceiveTimeoutMs = 4000,
    [string]$OutputJsonPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$targets = @(
    @{
        Name = "rust-combined-server"
        Uri = "ws://127.0.0.1:7878/ws/bench-room"
        Mode = "rust"
    },
    @{
        Name = "rust-integrated-hosted-server"
        Uri = "ws://127.0.0.1:7979/ws/bench-room"
        Mode = "rust"
    },
    @{
        Name = "dotnet-host-runtime"
        Uri = "ws://127.0.0.1:8787/ws/runtime"
        Mode = "dotnet"
    }
)

function New-BenchmarkPubkey {
    param([string]$Seed)

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Seed)
    $hashBytes = [System.Security.Cryptography.SHA256]::HashData($bytes)
    return [System.Convert]::ToHexString($hashBytes).ToLowerInvariant()
}

function Get-Percentile {
    param(
        [double[]]$Values,
        [double]$Percentile
    )

    if ($Values.Count -eq 0) {
        return [double]::NaN
    }

    $sorted = $Values | Sort-Object
    $index = [Math]::Ceiling(($Percentile / 100.0) * $sorted.Count) - 1
    if ($index -lt 0) { $index = 0 }
    if ($index -ge $sorted.Count) { $index = $sorted.Count - 1 }
    return [double]$sorted[$index]
}

function Invoke-BenchmarkRoundTrip {
    param(
        [string]$TargetName,
        [string]$Uri,
        [string]$Mode,
        [int]$Attempt,
        [int]$TimeoutMs
    )

    $ctSource = [System.Threading.CancellationTokenSource]::new()
    $ctSource.CancelAfter($TimeoutMs)
    $ct = $ctSource.Token

    $ws = [System.Net.WebSockets.ClientWebSocket]::new()
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $seed = "{0}:{1}:{2}:{3}" -f $TargetName, $Mode, $Attempt, [Guid]::NewGuid().ToString("N")
    $pubkey = New-BenchmarkPubkey -Seed $seed

    try {
        $null = $ws.ConnectAsync([Uri]$Uri, $ct).GetAwaiter().GetResult()

        if ($Mode -eq "rust") {
            $helloPayload = '{"type":"hello","room":"bench-room","pubkey":"' + $pubkey + '","frontier":[]}'
            $sendBytes = [System.Text.Encoding]::UTF8.GetBytes($helloPayload)
            $segment = [System.ArraySegment[byte]]::new($sendBytes, 0, $sendBytes.Length)
            $null = $ws.SendAsync($segment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).GetAwaiter().GetResult()
        }
        elseif ($Mode -eq "dotnet") {
            $helloPayload = '{"type":"hello","room":"bench-room","pubkey":"' + $pubkey + '","frontier":[]}'
            $helloBytes = [System.Text.Encoding]::UTF8.GetBytes($helloPayload)
            $helloSegment = [System.ArraySegment[byte]]::new($helloBytes, 0, $helloBytes.Length)
            $null = $ws.SendAsync($helloSegment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).GetAwaiter().GetResult()

            $noopBytes = [System.Text.Encoding]::UTF8.GetBytes('{"type":"noop"}')
            $noopSegment = [System.ArraySegment[byte]]::new($noopBytes, 0, $noopBytes.Length)
            $null = $ws.SendAsync($noopSegment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).GetAwaiter().GetResult()
        }
        else {
            throw "Unsupported benchmark mode '$Mode'"
        }

        $buffer = New-Object byte[] 8192
        $result = $ws.ReceiveAsync([System.ArraySegment[byte]]::new($buffer, 0, $buffer.Length), $ct).GetAwaiter().GetResult()

        if ($result.Count -le 0) {
            throw "No payload received"
        }

        $timer.Stop()
        return [double]$timer.Elapsed.TotalMilliseconds
    }
    finally {
        if ($ws.State -eq [System.Net.WebSockets.WebSocketState]::Open -or $ws.State -eq [System.Net.WebSockets.WebSocketState]::CloseReceived) {
            try {
                $null = $ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, "bench", [System.Threading.CancellationToken]::None).GetAwaiter().GetResult()
            }
            catch {
                # ignore close races
            }
        }

        $ws.Dispose()
        $ctSource.Dispose()
    }
}

function Test-TargetReachable {
    param(
        [string]$TargetName,
        [string]$Uri,
        [string]$Mode,
        [int]$TimeoutMs
    )

    try {
        [void](Invoke-BenchmarkRoundTrip -TargetName $TargetName -Uri $Uri -Mode $Mode -Attempt -1 -TimeoutMs $TimeoutMs)
        return $true
    }
    catch {
        return $false
    }
}

$allResults = @()

Write-Host "Benchmark configuration: warmup=$WarmupIterations measure=$MeasureIterations timeoutMs=$ReceiveTimeoutMs"

foreach ($target in $targets) {
    $name = [string]$target.Name
    $uri = [string]$target.Uri
    $mode = [string]$target.Mode

    Write-Host "`nTarget: $name ($uri) mode=$mode"

    if (-not (Test-TargetReachable -TargetName $name -Uri $uri -Mode $mode -TimeoutMs $ReceiveTimeoutMs)) {
        Write-Host "  status: SKIPPED (unreachable)"
        $allResults += [pscustomobject]@{
            target = $name
            uri = $uri
            mode = $mode
            status = "skipped-unreachable"
        }
        continue
    }

    for ($i = 0; $i -lt $WarmupIterations; $i++) {
        [void](Invoke-BenchmarkRoundTrip -TargetName $name -Uri $uri -Mode $mode -Attempt $i -TimeoutMs $ReceiveTimeoutMs)
    }

    $samples = New-Object System.Collections.Generic.List[double]
    for ($i = 0; $i -lt $MeasureIterations; $i++) {
        try {
            $latencyMs = Invoke-BenchmarkRoundTrip -TargetName $name -Uri $uri -Mode $mode -Attempt $i -TimeoutMs $ReceiveTimeoutMs
            [void]$samples.Add([double]$latencyMs)
        }
        catch {
            Write-Host "  iteration $i failed: $($_.Exception.Message)"
        }
    }

    if ($samples.Count -eq 0) {
        Write-Host "  status: ERROR (no successful samples)"
        $allResults += [pscustomobject]@{
            target = $name
            uri = $uri
            mode = $mode
            status = "error-no-samples"
        }
        continue
    }

    $values = $samples.ToArray()
    $avg = ($values | Measure-Object -Average).Average
    $min = ($values | Measure-Object -Minimum).Minimum
    $max = ($values | Measure-Object -Maximum).Maximum
    $p50 = Get-Percentile -Values $values -Percentile 50
    $p95 = Get-Percentile -Values $values -Percentile 95
    $p99 = Get-Percentile -Values $values -Percentile 99

    $rps = 0.0
    $sumMs = ($values | Measure-Object -Sum).Sum
    if ($sumMs -gt 0) {
        $rps = ($values.Count / $sumMs) * 1000.0
    }

    Write-Host ("  samples={0} avg={1:N2}ms p50={2:N2}ms p95={3:N2}ms p99={4:N2}ms min={5:N2}ms max={6:N2}ms approx_rps={7:N2}" -f $values.Count, $avg, $p50, $p95, $p99, $min, $max, $rps)

    $allResults += [pscustomobject]@{
        target = $name
        uri = $uri
        mode = $mode
        status = "ok"
        samples = $values.Count
        avg_ms = [Math]::Round([double]$avg, 3)
        p50_ms = [Math]::Round([double]$p50, 3)
        p95_ms = [Math]::Round([double]$p95, 3)
        p99_ms = [Math]::Round([double]$p99, 3)
        min_ms = [Math]::Round([double]$min, 3)
        max_ms = [Math]::Round([double]$max, 3)
        approx_rps = [Math]::Round([double]$rps, 3)
    }
}

if (-not [string]::IsNullOrWhiteSpace($OutputJsonPath)) {
    $json = $allResults | ConvertTo-Json -Depth 6
    $dir = Split-Path -Parent $OutputJsonPath
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path $dir)) {
        New-Item -Path $dir -ItemType Directory | Out-Null
    }
    Set-Content -Path $OutputJsonPath -Value $json -Encoding UTF8
    Write-Host "`nWrote results to $OutputJsonPath"
}

Write-Host "`nDone."
