using ActiveSync.DotNetHost;
using ActiveSync.DotNetHost.Ffi;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using System.Net;
using System.Net.Http;
using System.Net.Http.Json;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json.Nodes;
using System.Text.Json;

namespace ActiveSync.DotNetHost.Tests;

public sealed class SpeechSlateProxyAcceptanceTests
{
    [Fact]
    public async Task R19_SS_001_proxy_runtime_connect_and_dispatch_noop_with_mongo_s3delegated_profile()
    {
        await using var app = BuildTestApp(
            new ProxyDelegatedSuccessHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:NodeStorage"] = "Mongo",
                        ["ActiveSync:Storage:Mongo:ConnectionString"] = "mongodb://localhost:27017",
                        ["ActiveSync:Storage:Mongo:DatabaseName"] = "activesync-proxy-acceptance",
                        ["ActiveSync:Providers:BlobStorage"] = "S3Delegated",
                        ["ActiveSync:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["ActiveSync:Providers:Auth"] = "Default"
                    }
                );
            },
            services =>
            {
                services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
            }
        );
        await app.StartAsync();

        var wsClient = app.GetTestServer().CreateWebSocketClient();
        using var ws = await wsClient.ConnectAsync(new Uri("ws://localhost/ws/runtime"), CancellationToken.None);

        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");

        var frame = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", frame, StringComparison.Ordinal);
    }

    [Fact]
    public async Task R19_SS_002_proxy_blob_delegated_direct_path_returns_presigned_url()
    {
        await using var app = BuildTestApp(
            new ProxyDelegatedSuccessHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:BlobStorage"] = "S3Delegated",
                        ["ActiveSync:Storage:S3Delegated:BaseUrl"] = "https://delegate.test"
                    }
                );
            }
        );
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/sync/blob-url?op=get&room=room-a&namespace=assets&hash=sha256%3Aabc");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);

        Assert.Equal("https://delegate.example/presigned", doc.RootElement.GetProperty("url").GetString());
        Assert.Equal(1700000222L, doc.RootElement.GetProperty("expiresAt").GetInt64());
    }

    [Fact]
    public async Task R19_SS_002_proxy_blob_delegated_failure_falls_back_to_ws_path_contract_404()
    {
        await using var app = BuildTestApp(
            new ProxyDelegatedServerErrorHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:BlobStorage"] = "S3Delegated",
                        ["ActiveSync:Storage:S3Delegated:BaseUrl"] = "https://delegate.test"
                    }
                );
            }
        );
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/sync/blob-url?op=get&room=room-a&namespace=assets&hash=sha256%3Aabc");

        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    [Fact]
    public async Task R19_SS_003_proxy_auth_valid_embedded_token_allows_runtime_dispatch()
    {
        await using var app = BuildTestApp(
            new ProxyDelegatedSuccessHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:NodeStorage"] = "Mongo",
                        ["ActiveSync:Storage:Mongo:ConnectionString"] = "mongodb://localhost:27017",
                        ["ActiveSync:Storage:Mongo:DatabaseName"] = "activesync-proxy-acceptance",
                        ["ActiveSync:Providers:BlobStorage"] = "S3Delegated",
                        ["ActiveSync:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["ActiveSync:Providers:Auth"] = "JwtBridgeEmbedded",
                        ["ActiveSync:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                        ["ActiveSync:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                        ["ActiveSync:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                    }
                );
            },
            services =>
            {
                services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
            }
        );
        await app.StartAsync();

        var mintedToken = await MintTokenAsync(app, "room-auth", "peer-auth", ["read:**", "write:world/**"]);

        var wsClient = app.GetTestServer().CreateWebSocketClient();
        using var ws = await wsClient.ConnectAsync(new Uri("ws://localhost/ws/runtime"), CancellationToken.None);

        await SendTextAsync(
            ws,
            $"{{\"type\":\"hello\",\"room\":\"room-auth\",\"pubkey\":\"peer-auth\",\"frontier\":[],\"token\":{{\"peer_pubkey\":\"{mintedToken.PeerPubkeyHex}\",\"expiry\":{mintedToken.ExpirySecs},\"caps\":[\"read:**\",\"write:world/**\"],\"sig\":\"{mintedToken.SigHex}\"}}}}"
        );
        await SendTextAsync(ws, "{\"type\":\"noop\"}");

        var frame = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", frame, StringComparison.Ordinal);
    }

    [Fact]
    public async Task R19_SS_003_proxy_auth_policy_capability_mismatch_rejects_predictably()
    {
        await using var app = BuildTestApp(
            new ProxyDelegatedSuccessHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:NodeStorage"] = "Mongo",
                        ["ActiveSync:Storage:Mongo:ConnectionString"] = "mongodb://localhost:27017",
                        ["ActiveSync:Storage:Mongo:DatabaseName"] = "activesync-proxy-acceptance",
                        ["ActiveSync:Providers:BlobStorage"] = "S3Delegated",
                        ["ActiveSync:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["ActiveSync:Providers:Auth"] = "JwtBridgeEmbedded",
                        ["ActiveSync:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                        ["ActiveSync:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                        ["ActiveSync:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                    }
                );
            },
            services =>
            {
                services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
            }
        );
        await app.StartAsync();

        var mintedToken = await MintTokenAsync(app, "room-auth", "peer-auth", ["read:**"]);

        var wsClient = app.GetTestServer().CreateWebSocketClient();
        using var ws = await wsClient.ConnectAsync(new Uri("ws://localhost/ws/runtime"), CancellationToken.None);

        await SendTextAsync(
            ws,
            $"{{\"type\":\"hello\",\"room\":\"room-auth\",\"pubkey\":\"peer-auth\",\"frontier\":[],\"token\":{{\"peer_pubkey\":\"{mintedToken.PeerPubkeyHex}\",\"expiry\":{mintedToken.ExpirySecs},\"caps\":[\"read:**\",\"write:world/**\"],\"sig\":\"{mintedToken.SigHex}\"}}}}"
        );

        var frame = await ReceiveTextAsync(ws);
        Assert.Contains("token rejected", frame, StringComparison.Ordinal);
        Assert.Contains("capability mismatch", frame, StringComparison.Ordinal);
    }

    [Fact]
    public async Task R19_SS_004_proxy_transport_ws_first_remains_stable_with_optional_signaling_relay()
    {
        await using var app = BuildTestApp(
            new ProxyDelegatedSuccessHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:NodeStorage"] = "Mongo",
                        ["ActiveSync:Storage:Mongo:ConnectionString"] = "mongodb://localhost:27017",
                        ["ActiveSync:Storage:Mongo:DatabaseName"] = "activesync-proxy-acceptance",
                        ["ActiveSync:Providers:BlobStorage"] = "S3Delegated",
                        ["ActiveSync:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["ActiveSync:Providers:Auth"] = "Default"
                    }
                );
            },
            services =>
            {
                services.AddSingleton<IRuntimeCommandBridge>(new SignalingAndNoopRuntimeCommandBridge());
            }
        );
        await app.StartAsync();

        var wsClient = app.GetTestServer().CreateWebSocketClient();
        using var wsA = await wsClient.ConnectAsync(new Uri("ws://localhost/ws/runtime"), CancellationToken.None);
        using var wsB = await wsClient.ConnectAsync(new Uri("ws://localhost/ws/runtime"), CancellationToken.None);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-transport\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-transport\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        _ = await TryReceiveTextAsync(wsA, TimeSpan.FromMilliseconds(250));

        await SendTextAsync(wsA, "{\"type\":\"webrtc-offer\",\"to\":\"peer-b\",\"sdp\":{\"type\":\"offer\",\"sdp\":\"v=0\"}}");

        var relayed = await ReceiveUntilContainsAsync(wsB, "\"type\":\"webrtc-offer\"", TimeSpan.FromMilliseconds(250), 6);
        Assert.NotNull(relayed);
        Assert.Contains("\"from\":\"peer-a\"", relayed!, StringComparison.Ordinal);
        Assert.Contains("\"to\":\"peer-b\"", relayed!, StringComparison.Ordinal);

        await SendTextAsync(wsA, "{\"type\":\"noop\"}");
        await SendTextAsync(wsB, "{\"type\":\"noop\"}");

        var ackA = await ReceiveUntilContainsAsync(wsA, "\"type\":\"noop-ack\"", TimeSpan.FromMilliseconds(250), 6);
        var ackB = await ReceiveUntilContainsAsync(wsB, "\"type\":\"noop-ack\"", TimeSpan.FromMilliseconds(250), 6);

        Assert.NotNull(ackA);
        Assert.NotNull(ackB);
    }

    private static WebApplication BuildTestApp(
        HttpMessageHandler delegatedHandler,
        Action<IConfigurationBuilder> configureConfiguration,
        Action<IServiceCollection>? configureServices = null
    )
    {
        return HostApplication.Build(
            [],
            configureServices: services =>
            {
                services.AddSingleton<IHttpClientFactory>(new ProxyDelegatedHttpClientFactory(delegatedHandler));
                configureServices?.Invoke(services);
            },
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg => configureConfiguration(cfg)
        );
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

    private static async Task<string?> TryReceiveTextAsync(WebSocket ws, TimeSpan timeout)
    {
        using var cts = new CancellationTokenSource(timeout);
        try
        {
            return await ReceiveTextAsync(ws).WaitAsync(cts.Token);
        }
        catch (OperationCanceledException)
        {
            return null;
        }
    }

    private static async Task<string?> ReceiveUntilContainsAsync(
        WebSocket ws,
        string expected,
        TimeSpan perAttemptTimeout,
        int maxAttempts
    )
    {
        for (var attempt = 0; attempt < maxAttempts; attempt++)
        {
            var frame = await TryReceiveTextAsync(ws, perAttemptTimeout);
            if (!string.IsNullOrWhiteSpace(frame) && frame.Contains(expected, StringComparison.Ordinal))
            {
                return frame;
            }
        }

        return null;
    }

    private static async Task<MintedToken> MintTokenAsync(
        WebApplication app,
        string room,
        string peerPubkeyHex,
        IReadOnlyList<string> capabilities
    )
    {
        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room,
                peerPubkeyHex,
                lifetimeSeconds = 120,
                capabilities
            }
        );

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);

        await using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);

        return new MintedToken(
            doc.RootElement.GetProperty("peer_pubkey_hex").GetString() ?? string.Empty,
            doc.RootElement.GetProperty("expiry_secs").GetInt64(),
            doc.RootElement.GetProperty("sig_hex").GetString() ?? string.Empty
        );
    }

    private sealed record MintedToken(string PeerPubkeyHex, long ExpirySecs, string SigHex);

    private sealed class ProxyDelegatedHttpClientFactory : IHttpClientFactory
    {
        private readonly HttpClient _client;

        public ProxyDelegatedHttpClientFactory(HttpMessageHandler handler)
        {
            _client = new HttpClient(handler)
            {
                BaseAddress = new Uri("https://delegate.test")
            };
        }

        public HttpClient CreateClient(string name) => _client;
    }

    private sealed class ProxyDelegatedSuccessHandler : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent("{\"url\":\"https://delegate.example/presigned\",\"expires_at_epoch_seconds\":1700000222}", Encoding.UTF8, "application/json")
            });
        }
    }

    private sealed class ProxyDelegatedServerErrorHandler : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.InternalServerError)
            {
                Content = new StringContent("delegate failure", Encoding.UTF8, "text/plain")
            });
        }
    }

    private sealed class SignalingAndNoopRuntimeCommandBridge : IRuntimeCommandBridge
    {
        public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
        {
            JsonNode? root;
            try
            {
                root = JsonNode.Parse(commandJson);
            }
            catch
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            var commandNode = root?["command"];
            if (commandNode is null)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            if (commandNode.GetValueKind() == JsonValueKind.String
                && string.Equals(commandNode.GetValue<string>(), "Noop", StringComparison.Ordinal))
            {
                return FfiJsonBridgeResult.Success("[\"NoopAck\"]");
            }

            if (commandNode is JsonObject commandObject
                && commandObject["RelayPeerSignal"] is JsonObject relay
                && relay["msg_type"]?.GetValue<string>() is { } msgType
                && relay["to_peer_pubkey"]?.GetValue<string>() is { } toPeer)
            {
                var fromPeer = "peer-a";
                var roomId = root?["room_id"]?.GetValue<string>() ?? "room-transport";
                var payload = relay["payload"]?.ToJsonString() ?? "{}";

                var eventJson =
                    "[{\"PeerSignalRelayed\":{\"room_id\":\"" + roomId +
                    "\",\"from_peer_pubkey\":\"" + fromPeer +
                    "\",\"msg_type\":\"" + msgType +
                    "\",\"to_peer_pubkey\":\"" + toPeer +
                    "\",\"payload\":" + payload + "}}]";

                return FfiJsonBridgeResult.Success(eventJson);
            }

            return FfiJsonBridgeResult.Success("[]");
        }
    }
}
