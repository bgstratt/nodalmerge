using System.Text.Json;
using System.Text.Json.Nodes;

namespace NodalMerge.DotNetHost.Runtime;

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
        return BuildStatusError(status, null, null, null, null);
    }

    public static string BuildStatusError(
        string status,
        string? message,
        string? reasonClass,
        string? command,
        string? requiredCapability
    )
    {
        var error = new JsonObject
        {
            ["type"] = "error",
            ["status"] = status
        };

        if (!string.IsNullOrWhiteSpace(message))
        {
            error["msg"] = message;
        }

        if (!string.IsNullOrWhiteSpace(reasonClass))
        {
            error["reason_class"] = reasonClass;
        }

        if (!string.IsNullOrWhiteSpace(command))
        {
            error["command"] = command;
        }

        if (!string.IsNullOrWhiteSpace(requiredCapability))
        {
            error["required_capability"] = requiredCapability;
        }

        return error.ToJsonString();
    }
}