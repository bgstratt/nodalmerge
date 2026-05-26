using NodalMerge.DotNetHost.Runtime;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public class RuntimeErrorEnvelopeBuilderTests
{
    [Fact]
    public void Build_message_error_serializes_type_and_msg()
    {
        var json = RuntimeErrorEnvelopeBuilder.BuildMessageError("text messages required");
        using var doc = JsonDocument.Parse(json);

        Assert.Equal("error", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal(
            "text messages required",
            doc.RootElement.GetProperty("msg").GetString()
        );
    }

    [Fact]
    public void Build_message_error_escapes_special_characters()
    {
        var message = "invalid JSON: \"oops\"\nretry";
        var json = RuntimeErrorEnvelopeBuilder.BuildMessageError(message);
        using var doc = JsonDocument.Parse(json);

        Assert.Equal(message, doc.RootElement.GetProperty("msg").GetString());
    }

    [Fact]
    public void Build_status_error_serializes_type_and_status()
    {
        var json = RuntimeErrorEnvelopeBuilder.BuildStatusError("InvalidArg");
        using var doc = JsonDocument.Parse(json);

        Assert.Equal("error", doc.RootElement.GetProperty("type").GetString());
        Assert.Equal("InvalidArg", doc.RootElement.GetProperty("status").GetString());
    }
}