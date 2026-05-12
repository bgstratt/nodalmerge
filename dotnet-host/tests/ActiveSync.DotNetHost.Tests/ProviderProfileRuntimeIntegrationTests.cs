using ActiveSync.DotNetHost;
using ActiveSync.DotNetHost.Ffi;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using System.Net.WebSockets;
using System.Text;

namespace ActiveSync.DotNetHost.Tests;

public sealed class ProviderProfileRuntimeIntegrationTests
{
    [Fact]
    public async Task Runtime_hello_with_token_is_accepted_under_default_auth_profile()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "Default"
                }
            );
        }, services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":4102444800,\"caps\":[\"read:**\"],\"sig\":\"not-embedded\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"noop\"}");

        var outbound = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", outbound, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Runtime_hello_with_token_is_rejected_under_jwt_bridge_embedded_profile()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["ActiveSync:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["ActiveSync:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["ActiveSync:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                }
            );
        }, services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":4102444800,\"caps\":[\"read:**\"],\"sig\":\"not-embedded\"}}"
        );

        var tokenError = await ReceiveTextAsync(ws);
        Assert.Contains("token rejected", tokenError, StringComparison.Ordinal);
    }

    private static WebApplication BuildTestApp(
        Action<IConfigurationBuilder>? configureConfiguration = null,
        Action<IServiceCollection>? configureServices = null
    )
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg => configureConfiguration?.Invoke(cfg)
        );
    }

    private static async Task<WebSocket> ConnectRuntimeWebSocketAsync(WebApplication app)
    {
        var client = app.GetTestServer().CreateWebSocketClient();
        return await client.ConnectAsync(new Uri("ws://localhost/ws/runtime"), CancellationToken.None);
    }

    private static async Task SendTextAsync(WebSocket ws, string text)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        await ws.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
    }

    private static async Task<string> ReceiveTextAsync(WebSocket ws)
    {
        var buffer = new byte[8 * 1024];
        using var ms = new MemoryStream();
        WebSocketReceiveResult result;

        do
        {
            result = await ws.ReceiveAsync(buffer, CancellationToken.None);
            if (result.Count > 0)
            {
                await ms.WriteAsync(buffer.AsMemory(0, result.Count));
            }
        }
        while (!result.EndOfMessage);

        Assert.Equal(WebSocketMessageType.Text, result.MessageType);
        return Encoding.UTF8.GetString(ms.ToArray());
    }
}
