using ActiveSync.DotNetHost.Runtime;
using System.Diagnostics.Metrics;
using System.Net.WebSockets;
using System.Text;

namespace ActiveSync.DotNetHost.Ffi;

public sealed class FfiWebSocketLoopRunner
{
    private static readonly Meter RuntimeControlPlaneMeter = new("ActiveSync.DotNetHost.RuntimeControlPlane", "1.0.0");
    private static readonly Counter<long> RuntimeControlPlaneDeniedCounter = RuntimeControlPlaneMeter.CreateCounter<long>(
        "runtime_control_plane_denied_total"
    );

    public const int MaxInboundMessageBytes = 64 * 1024;

    public async Task RunAsync(
        WebSocket socket,
        IFfiBinaryBridge bridge,
        CancellationToken cancellationToken = default
    )
    {
        try
        {
            var frameBuffer = new byte[64 * 1024];

            while (socket.State == WebSocketState.Open)
            {
                using var messageBuffer = new MemoryStream();
                WebSocketReceiveResult? result;
                var messageTooLarge = false;

                do
                {
                    result = await socket.ReceiveAsync(frameBuffer, cancellationToken);

                    if (result.MessageType == WebSocketMessageType.Close)
                    {
                        await TryCloseNormalAsync(socket, "client requested close", cancellationToken);
                        return;
                    }

                    if (result.Count > 0)
                    {
                        if (messageBuffer.Length + result.Count > MaxInboundMessageBytes)
                        {
                            messageTooLarge = true;
                        }
                        else
                        {
                            await messageBuffer.WriteAsync(frameBuffer.AsMemory(0, result.Count), cancellationToken);
                        }
                    }
                }
                while (!result.EndOfMessage);

                if (messageTooLarge)
                {
                    var tooLargeBytes = Encoding.UTF8.GetBytes(
                        RuntimeErrorEnvelopeBuilder.BuildMessageError("message too large")
                    );
                    var sentTooLarge = await TrySendAsync(
                        socket,
                        tooLargeBytes,
                        WebSocketMessageType.Text,
                        cancellationToken
                    );
                    if (!sentTooLarge)
                    {
                        return;
                    }
                    continue;
                }

                if (result.MessageType != WebSocketMessageType.Binary)
                {
                    var invalidTypeBytes = Encoding.UTF8.GetBytes(
                        RuntimeErrorEnvelopeBuilder.BuildMessageError("binary messages required")
                    );
                    var sentInvalidType = await TrySendAsync(
                        socket,
                        invalidTypeBytes,
                        WebSocketMessageType.Text,
                        cancellationToken
                    );
                    if (!sentInvalidType)
                    {
                        return;
                    }
                    continue;
                }

                var commandPayload = messageBuffer.ToArray();
                var bridgeResult = bridge.ProcessBinaryCommand(commandPayload);

                if (bridgeResult.IsSuccess)
                {
                    var sentEvents = await TrySendAsync(
                        socket,
                        bridgeResult.EventsPayload,
                        WebSocketMessageType.Binary,
                        cancellationToken
                    );
                    if (!sentEvents)
                    {
                        return;
                    }
                    continue;
                }

                RecordControlPlaneDenyMetricIfApplicable(bridgeResult);
                var errorText = RuntimeErrorEnvelopeBuilder.BuildStatusError(bridgeResult.Status.ToString());
                var errorBytes = Encoding.UTF8.GetBytes(errorText);
                var sentStatusError = await TrySendAsync(
                    socket,
                    errorBytes,
                    WebSocketMessageType.Text,
                    cancellationToken
                );
                if (!sentStatusError)
                {
                    return;
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Host shutdown/request abort cancellation should end the loop cleanly.
        }
        catch (WebSocketException)
        {
            // Transport-level receive/send race should end the loop cleanly.
        }
        catch (ObjectDisposedException)
        {
            // Socket was disposed during shutdown/disconnect race.
        }
    }

    private static void RecordControlPlaneDenyMetricIfApplicable(FfiBridgeResult bridgeResult)
    {
        if (bridgeResult.Status != AsStatus.Policy)
        {
            return;
        }

        var command = string.IsNullOrWhiteSpace(bridgeResult.DenyMetadata?.Command)
            ? "ffi"
            : bridgeResult.DenyMetadata!.Command;
        var requiredCapability = string.IsNullOrWhiteSpace(bridgeResult.DenyMetadata?.RequiredCapability)
            ? "unknown"
            : bridgeResult.DenyMetadata!.RequiredCapability;
        var reasonClass = string.IsNullOrWhiteSpace(bridgeResult.DenyMetadata?.ReasonClass)
            ? "reject.control_plane_forbidden"
            : bridgeResult.DenyMetadata!.ReasonClass;

        RuntimeControlPlaneDeniedCounter.Add(
            1,
            KeyValuePair.Create<string, object?>("host", "dotnet-host"),
            KeyValuePair.Create<string, object?>("command", command),
            KeyValuePair.Create<string, object?>("required_capability", requiredCapability),
            KeyValuePair.Create<string, object?>("reason_class", reasonClass)
        );
    }

    private static async Task<bool> TrySendAsync(
        WebSocket socket,
        byte[] payload,
        WebSocketMessageType messageType,
        CancellationToken cancellationToken
    )
    {
        if (socket.State != WebSocketState.Open)
        {
            return false;
        }

        try
        {
            await socket.SendAsync(payload, messageType, true, cancellationToken);
            return true;
        }
        catch (WebSocketException)
        {
            return false;
        }
        catch (ObjectDisposedException)
        {
            return false;
        }
    }

    private static async Task TryCloseNormalAsync(WebSocket socket, string description, CancellationToken cancellationToken)
    {
        if (socket.State != WebSocketState.Open && socket.State != WebSocketState.CloseReceived)
        {
            return;
        }

        try
        {
            await socket.CloseAsync(WebSocketCloseStatus.NormalClosure, description, cancellationToken);
        }
        catch (WebSocketException)
        {
            // Connection already closed by peer/transport.
        }
        catch (ObjectDisposedException)
        {
            // Socket disposed during shutdown race.
        }
    }
}
