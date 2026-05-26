[CmdletBinding()]
param(
    [string]$BaseUrl = "http://127.0.0.1:8787",
    [string]$DelegateBaseUrl = "http://127.0.0.1:8788",
    [int]$StartupTimeoutSeconds = 45,
    [switch]$UseMongo,
    [string]$MongoConnectionString = "",
    [string]$MongoDatabaseName = "",
    [switch]$UseNuGetPackages,
    [string]$NodalMergePackageVersion = "0.1.0-local",
    [Alias("NodalMergePackageVersion")]
    [string]$LegacyNodalMergePackageVersion = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-FfiDllPath {
    $candidates = @(
        (Join-Path $PSScriptRoot "..\target\debug\nodalmerge_host_ffi.dll"),
        (Join-Path $PSScriptRoot "..\target\release\nodalmerge_host_ffi.dll")
    )

    foreach ($candidate in $candidates) {
        $full = [System.IO.Path]::GetFullPath($candidate)
        if (Test-Path $full) {
            return $full
        }
    }

    return $null
}

function Start-DelegatedStub {
    param([string]$ListenBase)

    Write-Host "Starting delegated stub at $ListenBase ..."
    return Start-Job -ArgumentList $ListenBase -ScriptBlock {
        param($JobListenBase)

        $listener = [System.Net.HttpListener]::new()
        $prefix = if ($JobListenBase.EndsWith('/')) { $JobListenBase } else { "$JobListenBase/" }
        $listener.Prefixes.Add($prefix)
        $listener.Start()

        try {
            while ($listener.IsListening) {
                try {
                    $ctx = $listener.GetContext()
                }
                catch {
                    break
                }

                $requestPath = $ctx.Request.Url.AbsolutePath
                $requestBody = ""
                $reader = [System.IO.StreamReader]::new($ctx.Request.InputStream, [System.Text.Encoding]::UTF8)
                try {
                    $requestBody = $reader.ReadToEnd()
                }
                finally {
                    $reader.Dispose()
                }
                $response = $ctx.Response
                $response.ContentType = "application/json"

                if ($ctx.Request.HttpMethod -ne "POST") {
                    $response.StatusCode = 405
                    $payload = '{"error":"method-not-allowed"}'
                }
                elseif ($requestPath -eq "/v1/blobs/presign-get") {
                    Write-Host "[delegated-stub] GET presign request received: $requestBody"
                    $response.StatusCode = 200
                    $payload = '{"url":"https://delegate.example/get/sha256-abc","expiresAtEpochSeconds":1700001111}'
                }
                elseif ($requestPath -eq "/v1/blobs/presign-put") {
                    Write-Host "[delegated-stub] PUT presign request received: $requestBody"
                    $response.StatusCode = 200
                    $payload = '{"url":"https://delegate.example/put/sha256-abc","expiresAtEpochSeconds":1700002222}'
                }
                else {
                    $response.StatusCode = 404
                    $payload = '{"error":"not-found"}'
                }

                $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
                $response.ContentLength64 = $bytes.Length
                $response.OutputStream.Write($bytes, 0, $bytes.Length)
                $response.OutputStream.Close()
            }
        }
        finally {
            if ($listener.IsListening) {
                $listener.Stop()
            }
            $listener.Close()
        }
    }
}

function Assert-DelegatedStubReady {
    param([string]$ListenBase)

    $probeUri = "$ListenBase/v1/blobs/presign-get"
    $probeBody = '{"room":"room-probe","namespace":"assets","hash":"sha256:probe"}'
    $start = Get-Date
    while (((Get-Date) - $start).TotalSeconds -lt 10) {
        try {
            $resp = Invoke-RestMethod -Uri $probeUri -Method POST -ContentType "application/json" -Body $probeBody
            if ($resp.url -like "https://delegate.example/*") {
                return
            }
        }
        catch {
            Start-Sleep -Milliseconds 250
        }
    }

    throw "Delegated stub did not become ready at $probeUri"
}

$wsUrl = $BaseUrl.Replace("http://", "ws://").Replace("https://", "wss://") + "/ws/runtime"
$delegateStub = $null
$process = $null
$blobCheckPassed = $false
$wsCheckPassed = $false

try {
    $delegateStub = Start-DelegatedStub -ListenBase $DelegateBaseUrl
    Assert-DelegatedStubReady -ListenBase $DelegateBaseUrl

    $hostEnv = @{
        ASPNETCORE_URLS = $BaseUrl
    }

    if ($UseNuGetPackages.IsPresent) {
        Write-Host "Using package mode; native runtime should resolve from NuGet runtime assets."
    }
    elseif ([string]::IsNullOrWhiteSpace($env:NODALMERGE_HOST_FFI_DLL) -and [string]::IsNullOrWhiteSpace($env:ACTIVESYNC_HOST_FFI_DLL)) {
        $resolvedFfiDll = Resolve-FfiDllPath
        if ($resolvedFfiDll) {
            $hostEnv["NODALMERGE_HOST_FFI_DLL"] = $resolvedFfiDll
            Write-Host "Using NODALMERGE_HOST_FFI_DLL=$resolvedFfiDll"
        }
        else {
            Write-Host "Warning: NODALMERGE_HOST_FFI_DLL/ACTIVESYNC_HOST_FFI_DLL is not set and no local host-ffi DLL was found."
            Write-Host "Build with: cargo build -p nodalmerge-host-ffi"
        }
    }
    elseif (-not [string]::IsNullOrWhiteSpace($env:NODALMERGE_HOST_FFI_DLL)) {
        $hostEnv["NODALMERGE_HOST_FFI_DLL"] = $env:NODALMERGE_HOST_FFI_DLL
        Write-Host "Using NODALMERGE_HOST_FFI_DLL from current environment"
    }
    else {
        $hostEnv["ACTIVESYNC_HOST_FFI_DLL"] = $env:ACTIVESYNC_HOST_FFI_DLL
        Write-Host "Using ACTIVESYNC_HOST_FFI_DLL from current environment"
    }

    $resolvedPackageVersion = if (-not [string]::IsNullOrWhiteSpace($LegacyNodalMergePackageVersion)) {
        $LegacyNodalMergePackageVersion
    }
    else {
        $NodalMergePackageVersion
    }

    if ($UseNuGetPackages.IsPresent) {
        $restoreArgs = @(
            "restore",
            "./NodalMerge.DotNetHost.slnx",
            "--configfile", "./NuGet.Local.config",
            "-p:NodalMergeUseNuGetPackages=true",
            "-p:NodalMergePackageVersion=$resolvedPackageVersion"
        )

        Write-Host "Restoring in package mode with NuGet.Local.config ..."
        & dotnet @restoreArgs
        if ($LASTEXITCODE -ne 0) {
            throw "dotnet restore failed in package mode"
        }
    }

    $runtimeArgs = @(
        "--NodalMerge:Providers:BlobStorage=S3Delegated",
        "--NodalMerge:Storage:S3Delegated:BaseUrl=$DelegateBaseUrl",
        "--NodalMerge:Storage:S3Delegated:PutPath=/v1/blobs/presign-put",
        "--NodalMerge:Storage:S3Delegated:GetPath=/v1/blobs/presign-get",
        "--NodalMerge:Storage:S3Delegated:TimeoutSeconds=5",
        "--NodalMerge:Storage:S3Delegated:MaxRetries=2",
        "--NodalMerge:Storage:S3Delegated:CircuitBreakerFailureThreshold=3",
        "--NodalMerge:Storage:S3Delegated:CircuitBreakerOpenSeconds=30"
    )

    if ($UseMongo.IsPresent) {
        $runtimeArgs += "--NodalMerge:Providers:NodeStorage=Mongo"
        if (-not [string]::IsNullOrWhiteSpace($MongoConnectionString)) {
            $runtimeArgs += "--NodalMerge:Storage:Mongo:ConnectionString=$MongoConnectionString"
        }
        if (-not [string]::IsNullOrWhiteSpace($MongoDatabaseName)) {
            $runtimeArgs += "--NodalMerge:Storage:Mongo:DatabaseName=$MongoDatabaseName"
        }
    }
    else {
        $runtimeArgs += "--NodalMerge:Providers:NodeStorage=InMemory"
    }

    $hostArgs = @(
        "run",
        "--project", "src/ActiveSync.DotNetHost/NodalMerge.DotNetHost.csproj",
        "--no-launch-profile"
    )

    if ($UseNuGetPackages.IsPresent) {
        $hostArgs += @(
            "-p:NodalMergeUseNuGetPackages=true",
            "-p:NodalMergePackageVersion=$resolvedPackageVersion"
        )
    }

    $hostArgs += "--"
    $hostArgs += $runtimeArgs

    Write-Host "Starting Host with delegated blob profile..."
    $previousHostEnv = @{}
    foreach ($entry in $hostEnv.GetEnumerator()) {
        $previousHostEnv[$entry.Key] = [System.Environment]::GetEnvironmentVariable($entry.Key, [System.EnvironmentVariableTarget]::Process)
        [System.Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, [System.EnvironmentVariableTarget]::Process)
    }

    try {
        $process = Start-Process dotnet -ArgumentList $hostArgs -NoNewWindow -PassThru
    }
    finally {
        foreach ($entry in $previousHostEnv.GetEnumerator()) {
            [System.Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, [System.EnvironmentVariableTarget]::Process)
        }
    }

    $start = Get-Date
    $ready = $false
    while (((Get-Date) - $start).TotalSeconds -lt $StartupTimeoutSeconds) {
        try {
            $resp = Invoke-WebRequest -Uri "$BaseUrl/ffi/abi-version" -UseBasicParsing
            if ($resp.StatusCode -eq 200) {
                $ready = $true
                break
            }
        }
        catch {
            Start-Sleep -Milliseconds 750
        }
    }

    if (-not $ready) {
        throw "Server failed to respond within $StartupTimeoutSeconds seconds"
    }

    Write-Host "Server ready. Verifying delegated blob-url route..."

    $blobUris = @(
        "$BaseUrl/sync/blob-url?op=get&room=room-verify&namespace=assets&hash=sha256%3Aabc",
        "$BaseUrl/api/sync/blob-url?op=get&room=room-verify&namespace=assets&hash=sha256%3Aabc",
        "$BaseUrl/sync/blob-url?op=put&room=room-verify&namespace=assets&hash=sha256%3Aabc&size=128&contentType=application%2Foctet-stream",
        "$BaseUrl/api/sync/blob-url?op=put&room=room-verify&namespace=assets&hash=sha256%3Aabc&size=128&contentType=application%2Foctet-stream"
    )

    $blobResp = $null
    foreach ($blobUri in $blobUris) {
        try {
            $blobResp = Invoke-RestMethod -Uri $blobUri -Method GET -ErrorAction Stop
            Write-Host "Blob URL route responded on $blobUri"
            if ($blobResp.url) {
                break
            }
        }
        catch {
            $statusCode = $_.Exception.Response.StatusCode.value__
            Write-Host "Blob URL route $blobUri returned status $statusCode"
        }
    }

    if ($blobResp -and $blobResp.url -and $blobResp.url -like "https://delegate.example/*") {
        $blobCheckPassed = $true
        Write-Host "Delegated blob-url check passed: $($blobResp.url)"
    }
    else {
        Write-Host "Delegated blob-url check failed"
    }

    Write-Host "Verifying runtime websocket hello/noop flow..."
    $ws = [System.Net.WebSockets.ClientWebSocket]::new()
    $ct = [System.Threading.CancellationToken]::None
    $ws.ConnectAsync([System.Uri]::new($wsUrl), $ct).GetAwaiter().GetResult()

    $hello = '{"type":"hello","room":"room-verify","pubkey":"peer-verify","frontier":[]}'
    $noop = '{"type":"noop"}'

    foreach ($frame in @($hello, $noop)) {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($frame)
        $segment = [System.ArraySegment[byte]]::new($bytes, 0, $bytes.Length)
        $ws.SendAsync($segment, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).GetAwaiter().GetResult()
    }

    $buffer = New-Object byte[] 4096
    $foundNoopAck = $false
    for ($i = 0; $i -lt 5; $i++) {
        $segment = [System.ArraySegment[byte]]::new($buffer, 0, $buffer.Length)
        $result = $ws.ReceiveAsync($segment, $ct).GetAwaiter().GetResult()
        $responseText = [System.Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count)
        Write-Host "Received: $responseText"
        if ($responseText -like "*noop-ack*") {
            $foundNoopAck = $true
            break
        }
    }

    if (-not $foundNoopAck) {
        throw "WebSocket verification failed: noop-ack not found"
    }

    $wsCheckPassed = $true

    $ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, "Done", $ct).GetAwaiter().GetResult()
    if ($blobCheckPassed -and $wsCheckPassed) {
        Write-Host "Verification SUCCESS (delegated blob-url + runtime websocket)."
    }
    elseif ($wsCheckPassed) {
        throw "Verification partial: websocket passed but delegated blob-url failed"
    }
    else {
        throw "Verification failed"
    }
}
finally {
    if ($process -and -not $process.HasExited) {
        Write-Host "Stopping Host..."
        Stop-Process -Id $process.Id -Force
    }

    if ($delegateStub) {
        Write-Host "Stopping delegated stub..."
        Stop-Job -Id $delegateStub.Id -ErrorAction SilentlyContinue
        Remove-Job -Id $delegateStub.Id -Force -ErrorAction SilentlyContinue
    }
}


