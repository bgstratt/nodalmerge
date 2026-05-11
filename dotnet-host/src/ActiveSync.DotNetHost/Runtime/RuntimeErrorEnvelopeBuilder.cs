using System.Text.Json;

namespace ActiveSync.DotNetHost.Runtime;

public static class RuntimeErrorEnvelopeBuilder
{
    public static string BuildMessageError(string message)
    {
        return JsonSerializer.Serialize(new
        {
            type = "error",
            msg = message
        });
    }

    public static string BuildStatusError(string status)
    {
        return JsonSerializer.Serialize(new
        {
            type = "error",
            status
        });
    }
}