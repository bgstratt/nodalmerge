using System.Net.WebSockets;
using System.Text;

namespace NodalMerge.DotNetHost.Runtime;

public sealed class RuntimeFrameProcessor
{
    private readonly RuntimeMessageProcessor _messageProcessor;

    public RuntimeFrameProcessor(RuntimeMessageProcessor messageProcessor)
    {
        _messageProcessor = messageProcessor;
    }

    public RuntimeFrameProcessResult ProcessFrame(
        WebSocketMessageType messageType,
        byte[] payload,
        RuntimeConnectionState state
    )
    {
        if (messageType != WebSocketMessageType.Text)
        {
            return RuntimeFrameProcessResult.FromOutboundMessages(
                dispatchSucceeded: false,
                [RuntimeErrorEnvelopeBuilder.BuildMessageError("text messages required")],
                shouldCloseConnection: false
            );
        }

        var incomingJson = Encoding.UTF8.GetString(payload);
        var messageResult = _messageProcessor.ProcessIncomingText(incomingJson, state);
        return RuntimeFrameProcessResult.FromOutboundMessages(
            messageResult.DispatchSucceeded,
            messageResult.OutboundMessages,
            messageResult.ShouldCloseConnection
        );
    }
}

public sealed record RuntimeFrameProcessResult(
    bool DispatchSucceeded,
    IReadOnlyList<string> OutboundMessages,
    bool ShouldCloseConnection
)
{
    public static RuntimeFrameProcessResult FromOutboundMessages(
        bool dispatchSucceeded,
        IReadOnlyList<string> outboundMessages,
        bool shouldCloseConnection
    ) =>
        new(dispatchSucceeded, outboundMessages, shouldCloseConnection);
}