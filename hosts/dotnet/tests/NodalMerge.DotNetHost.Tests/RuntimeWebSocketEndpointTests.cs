using NodalMerge.DotNetHost;
using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Abstractions.Providers;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json.Nodes;

namespace NodalMerge.DotNetHost.Tests;

public class RuntimeWebSocketEndpointTests
{
    [Fact]
    public async Task Runtime_endpoint_non_websocket_request_returns_400()
    {
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/ws/runtime");

        Assert.Equal(System.Net.HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task Runtime_endpoint_hello_then_noop_emits_noop_ack()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");

        var outbound = await ReceiveTextAsync(ws);

        Assert.Contains("\"type\":\"noop-ack\"", outbound);
    }

    [Fact]
    public async Task Runtime_endpoint_room_path_alias_hello_then_noop_emits_noop_ack()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app, "/ws/default");
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"default\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");

        var outbound = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", outbound);
    }

    [Fact]
    public async Task Runtime_endpoint_close_session_success_closes_socket()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"close-session\"}");

        var closeFrame = await ReceiveFrameAsync(ws);

        Assert.Equal(WebSocketMessageType.Close, closeFrame.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeFrame.CloseStatus);
    }

    [Fact]
    public async Task Runtime_endpoint_fragmented_close_session_success_closes_socket()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextFragmentsAsync(ws, ["{\"type\":\"close-", "session\"}"]);

        var closeFrame = await ReceiveFrameAsync(ws);

        Assert.Equal(WebSocketMessageType.Close, closeFrame.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeFrame.CloseStatus);
    }

    [Fact]
    public async Task Runtime_endpoint_malformed_json_keeps_socket_open_for_follow_on_valid_messages()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(ws, "{not-json");
        var errorFrame = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"invalid JSON\"", errorFrame);

        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ackFrame = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ackFrame);
    }

    [Fact]
    public async Task Runtime_endpoint_duplicate_hello_returns_error_without_forcing_close()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        var hello = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}";

        await SendTextAsync(ws, hello);
        await SendTextAsync(ws, hello);
        var duplicateError = await TryReceiveTextAsync(ws, TimeSpan.FromMilliseconds(1000));
        Assert.NotNull(duplicateError);
        Assert.Contains("\"msg\":\"hello already processed for this connection\"", duplicateError);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var noopAck = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", noopAck);
    }

    [Fact]
    public async Task Runtime_endpoint_binary_frame_error_does_not_break_follow_on_noop()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");

        await ws.SendAsync(new byte[] { 0xAB }, WebSocketMessageType.Binary, true, CancellationToken.None);
        var typeError = await TryReceiveTextAsync(ws, TimeSpan.FromMilliseconds(1000));
        Assert.NotNull(typeError);
        Assert.Contains("\"msg\":\"text messages required\"", typeError);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_close_session_failure_keeps_connection_open_for_noop()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),   // hydrate pre-state EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hydrate pre-state RequestServerPack
                FfiJsonBridgeResult.Success("[]"),   // hydrate main EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hello EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hello OpenSession
                FfiJsonBridgeResult.Success("[]"),   // hello ClientHello
                FfiJsonBridgeResult.Success("[]"),   // hello RequestServerPack
                FfiJsonBridgeResult.Success("[]"),   // catch-up EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // catch-up RequestServerPack
                FfiJsonBridgeResult.Failure(AsStatus.Protocol),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"close-session\"}");

        var closeError = await ReceiveTextAsync(ws);
        Assert.Contains("\"status\":\"Protocol\"", closeError);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_fragmented_close_session_failure_keeps_connection_open_for_noop()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),   // hydrate pre-state EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hydrate pre-state RequestServerPack
                FfiJsonBridgeResult.Success("[]"),   // hydrate main EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hello EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hello OpenSession
                FfiJsonBridgeResult.Success("[]"),   // hello ClientHello
                FfiJsonBridgeResult.Success("[]"),   // hello RequestServerPack
                FfiJsonBridgeResult.Success("[]"),   // catch-up EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // catch-up RequestServerPack
                FfiJsonBridgeResult.Failure(AsStatus.Protocol),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextFragmentsAsync(ws, ["{\"type\":\"close-", "session\"}"]);

        var closeError = await ReceiveTextAsync(ws);
        Assert.Contains("\"status\":\"Protocol\"", closeError);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Theory]
    [InlineData(AsStatus.InvalidArg)]
    [InlineData(AsStatus.NotFound)]
    [InlineData(AsStatus.Auth)]
    [InlineData(AsStatus.Policy)]
    [InlineData(AsStatus.Protocol)]
    [InlineData(AsStatus.Internal)]
    public async Task Runtime_endpoint_close_session_status_matrix_returns_error_and_recovers(AsStatus status)
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),   // hydrate pre-state EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hydrate pre-state RequestServerPack
                FfiJsonBridgeResult.Success("[]"),   // hydrate main EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hello EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // hello OpenSession
                FfiJsonBridgeResult.Success("[]"),   // hello ClientHello
                FfiJsonBridgeResult.Success("[]"),   // hello RequestServerPack
                FfiJsonBridgeResult.Success("[]"),   // catch-up EnsureRoom
                FfiJsonBridgeResult.Success("[]"),   // catch-up RequestServerPack
                FfiJsonBridgeResult.Failure(status),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"close-session\"}");

        var closeError = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"error\"", closeError);
        Assert.Contains($"\"status\":\"{status}\"", closeError);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_oversized_message_returns_error_and_keeps_connection_open()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");

        var chunkA = new string('x', RuntimeWebSocketLoopRunner.MaxInboundMessageBytes);
        await SendTextFragmentsAsync(ws, [chunkA, "y"]);

        var sizeError = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"message too large\"", sizeError);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_client_abort_during_receive_allows_follow_on_connection()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using (var ws = await ConnectRuntimeWebSocketAsync(app))
        {
            await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
            ws.Abort();
        }

        using var ws2 = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws2, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");
        await SendTextAsync(ws2, "{\"type\":\"noop\"}");

        var ack = await ReceiveUntilContainsAsync(ws2, "\"type\":\"noop-ack\"", 4, TimeSpan.FromMilliseconds(500));
        Assert.NotNull(ack);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_partial_fragment_then_abort_allows_follow_on_connection()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using (var ws = await ConnectRuntimeWebSocketAsync(app))
        {
            await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
            var partial = Encoding.UTF8.GetBytes("{\"type\":\"noop\"");
            await ws.SendAsync(partial, WebSocketMessageType.Text, endOfMessage: false, CancellationToken.None);
            ws.Abort();
        }

        using var ws2 = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws2, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");
        await SendTextAsync(ws2, "{\"type\":\"noop\"}");

        var ack = await ReceiveUntilContainsAsync(ws2, "\"type\":\"noop-ack\"", 4, TimeSpan.FromMilliseconds(500));
        Assert.NotNull(ack);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_reconnect_soak_hello_noop_remains_stable()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        for (var i = 0; i < 3; i++)
        {
            using var ws = await ConnectRuntimeWebSocketAsync(app);
            await SendTextAsync(ws, $"{{\"type\":\"hello\",\"room\":\"room-{i}\",\"pubkey\":\"peer-{i}\",\"frontier\":[]}}");
            await SendTextAsync(ws, "{\"type\":\"noop\"}");

            var ack = await TryReceiveTextAsync(ws, TimeSpan.FromMilliseconds(1500));

            Assert.NotNull(ack);
            Assert.Contains("\"type\":\"noop-ack\"", ack);

            await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "soak-iteration-done", CancellationToken.None);
        }
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_connections_mixed_dispatch_outcomes_are_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new ParallelRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"session_id\":1001}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[],\"session_id\":2002}");

        await Task.WhenAll(
            SendTextAsync(wsA, "{\"type\":\"noop\"}"),
            SendTextAsync(wsB, "{\"type\":\"noop\"}")
        );

        var ackA = await ReceiveTextAsync(wsA);
        Assert.Contains("\"type\":\"noop-ack\"", ackA);

        var errorB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"error\"", errorB);
        Assert.Contains("\"status\":\"Protocol\"", errorB);

        await SendTextAsync(wsB, "{\"type\":\"noop\"}");
        var ackB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_churn_alternating_failures_remain_isolated_and_recover()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new AlternatingRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"session_id\":3001}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[],\"session_id\":4002}");

        var totalErrorCount = 0;
        for (var round = 0; round < 4; round++)
        {
            await Task.WhenAll(
                SendTextAsync(wsA, "{\"type\":\"noop\"}"),
                SendTextAsync(wsB, "{\"type\":\"noop\"}")
            );

            var frameA = await ReceiveTextAsync(wsA);
            var frameB = await ReceiveTextAsync(wsB);

            if (frameA.Contains("\"type\":\"error\"", StringComparison.Ordinal)
                && frameA.Contains("\"status\":\"Protocol\"", StringComparison.Ordinal))
            {
                totalErrorCount++;

                var recoveredA = false;
                for (var attempt = 0; attempt < 3; attempt++)
                {
                    await SendTextAsync(wsA, "{\"type\":\"noop\"}");
                    var recoveryA = await ReceiveTextAsync(wsA);
                    if (recoveryA.Contains("\"type\":\"noop-ack\"", StringComparison.Ordinal))
                    {
                        recoveredA = true;
                        break;
                    }

                    Assert.Contains("\"type\":\"error\"", recoveryA);
                    Assert.Contains("\"status\":\"Protocol\"", recoveryA);
                }

                Assert.True(recoveredA, "expected room-a socket to recover with noop-ack after protocol failures");
            }
            else
            {
                Assert.Contains("\"type\":\"noop-ack\"", frameA);
            }

            if (frameB.Contains("\"type\":\"error\"", StringComparison.Ordinal)
                && frameB.Contains("\"status\":\"Protocol\"", StringComparison.Ordinal))
            {
                totalErrorCount++;

                var recoveredB = false;
                for (var attempt = 0; attempt < 3; attempt++)
                {
                    await SendTextAsync(wsB, "{\"type\":\"noop\"}");
                    var recoveryB = await ReceiveTextAsync(wsB);
                    if (recoveryB.Contains("\"type\":\"noop-ack\"", StringComparison.Ordinal))
                    {
                        recoveredB = true;
                        break;
                    }

                    Assert.Contains("\"type\":\"error\"", recoveryB);
                    Assert.Contains("\"status\":\"Protocol\"", recoveryB);
                }

                Assert.True(recoveredB, "expected room-b socket to recover with noop-ack after protocol failures");
            }
            else
            {
                Assert.Contains("\"type\":\"noop-ack\"", frameB);
            }
        }

        Assert.True(totalErrorCount >= 2, "expected repeated protocol failures across churn rounds");
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_fragmented_churn_mixed_frames_remain_isolated_and_recover()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new AlternatingRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextFragmentsAsync(wsA, [
            "{\"type\":\"hello\",\"room\":\"room-a\",",
            "\"pubkey\":\"peer-a\",\"frontier\":[],\"session_id\":5001}"
        ]);
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[],\"session_id\":6002}");

        var totalErrorCount = 0;
        for (var round = 0; round < 6; round++)
        {
            await Task.WhenAll(
                SendTextFragmentsAsync(wsA, ["{\"type\":\"no", "op\"}"]),
                SendTextAsync(wsB, "{\"type\":\"noop\"}")
            );

            var frameA = await ReceiveTextAsync(wsA);
            var frameB = await ReceiveTextAsync(wsB);

            if (frameA.Contains("\"type\":\"error\"", StringComparison.Ordinal)
                && frameA.Contains("\"status\":\"Protocol\"", StringComparison.Ordinal))
            {
                totalErrorCount++;
                var recoveredA = false;
                for (var attempt = 0; attempt < 4; attempt++)
                {
                    await SendTextAsync(wsA, "{\"type\":\"noop\"}");
                    var recovery = await ReceiveTextAsync(wsA);
                    if (recovery.Contains("\"type\":\"noop-ack\"", StringComparison.Ordinal))
                    {
                        recoveredA = true;
                        break;
                    }

                    Assert.Contains("\"type\":\"error\"", recovery);
                    Assert.Contains("\"status\":\"Protocol\"", recovery);
                }

                Assert.True(recoveredA, "expected socket A to recover with noop-ack after protocol failures");
            }
            else
            {
                Assert.Contains("\"type\":\"noop-ack\"", frameA);
            }

            if (frameB.Contains("\"type\":\"error\"", StringComparison.Ordinal)
                && frameB.Contains("\"status\":\"Protocol\"", StringComparison.Ordinal))
            {
                totalErrorCount++;
                var recoveredB = false;
                for (var attempt = 0; attempt < 4; attempt++)
                {
                    await SendTextAsync(wsB, "{\"type\":\"noop\"}");
                    var recovery = await ReceiveTextAsync(wsB);
                    if (recovery.Contains("\"type\":\"noop-ack\"", StringComparison.Ordinal))
                    {
                        recoveredB = true;
                        break;
                    }

                    Assert.Contains("\"type\":\"error\"", recovery);
                    Assert.Contains("\"status\":\"Protocol\"", recovery);
                }

                Assert.True(recoveredB, "expected socket B to recover with noop-ack after protocol failures");
            }
            else
            {
                Assert.Contains("\"type\":\"noop-ack\"", frameB);
            }
        }

        Assert.True(totalErrorCount >= 3, "expected repeated protocol failures during fragmented churn rounds");
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_fragmented_oversized_interleaving_remains_isolated_and_recovers()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        var chunkA = new string('x', 32_000);
        var chunkB = new string('y', 32_000);
        var chunkC = new string('z', 1_537);

        for (var round = 0; round < 3; round++)
        {
            await Task.WhenAll(
                SendTextFragmentsAsync(wsA, [chunkA, chunkB, chunkC]),
                SendTextFragmentsAsync(wsB, ["{\"type\":\"no", "op\"}"])
            );

            var errorA = await ReceiveTextAsync(wsA);
            Assert.Contains("\"msg\":\"message too large\"", errorA);

            var ackB = await ReceiveTextAsync(wsB);
            Assert.Contains("\"type\":\"noop-ack\"", ackB);

            await SendTextAsync(wsA, "{\"type\":\"noop\"}");
            var recoveryA = await ReceiveTextAsync(wsA);
            Assert.Contains("\"type\":\"noop-ack\"", recoveryA);
        }
    }

    [Fact]
    public async Task Runtime_endpoint_client_close_output_triggers_normal_server_close()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "client half-close", CancellationToken.None);

        var closeFrame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Close, closeFrame.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeFrame.CloseStatus);
    }

    [Fact]
    public async Task Runtime_endpoint_partial_fragment_then_client_close_output_closes_cleanly()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        var partial = Encoding.UTF8.GetBytes("{\"type\":\"noop\"");
        await ws.SendAsync(partial, WebSocketMessageType.Text, endOfMessage: false, CancellationToken.None);
        await ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "client half-close after fragment", CancellationToken.None);

        var closeFrame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Close, closeFrame.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeFrame.CloseStatus);
    }

    [Fact]
    public async Task Runtime_endpoint_client_close_output_with_non_normal_status_still_closes_normally()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await ws.CloseOutputAsync(WebSocketCloseStatus.PolicyViolation, "policy-close", CancellationToken.None);

        var closeFrame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Close, closeFrame.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeFrame.CloseStatus);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_half_close_and_active_traffic_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendTextAsync(wsB, "{\"type\":\"noop\"}")
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeA.CloseStatus);

        var ackB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB);

        await SendTextAsync(wsB, "{\"type\":\"noop\"}");
        var ackB2 = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB2);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_half_close_and_active_error_recovery_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            wsB.SendAsync(new byte[] { 0xAA }, WebSocketMessageType.Binary, true, CancellationToken.None)
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeA.CloseStatus);

        var errorB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"msg\":\"text messages required\"", errorB);

        await SendTextAsync(wsB, "{\"type\":\"noop\"}");
        var ackB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_half_close_and_oversized_recovery_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");

        var chunkA = new string('x', 32_000);
        var chunkB = new string('y', 32_000);
        var chunkC = new string('z', 1_537);

        await Task.WhenAll(
            SendTextFragmentsAsync(wsA, [chunkA, chunkB, chunkC]),
            wsB.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None)
        );

        var errorA = await ReceiveTextAsync(wsA);
        Assert.Contains("\"msg\":\"message too large\"", errorA);

        await SendTextAsync(wsA, "{\"type\":\"noop\"}");
        var recoveryA = await ReceiveTextAsync(wsA);
        Assert.Contains("\"type\":\"noop-ack\"", recoveryA);

        var closeB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Close, closeB.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeB.CloseStatus);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_half_close_and_fragmented_active_success_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendTextFragmentsAsync(wsB, ["{\"type\":\"no", "op\"}"])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeA.CloseStatus);

        var ackB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB);

        await SendTextAsync(wsB, "{\"type\":\"noop\"}");
        var ackB2 = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB2);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_half_close_and_fragmented_active_error_recovery_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendBinaryFragmentsAsync(wsB, [new byte[] { 0x01 }, new byte[] { 0x02 }])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeA.CloseStatus);

        var errorB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"msg\":\"text messages required\"", errorB);

        await SendTextAsync(wsB, "{\"type\":\"noop\"}");
        var ackB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"type\":\"noop-ack\"", ackB);
    }

    [Fact]
    public async Task Runtime_endpoint_parallel_dual_half_close_with_third_active_recovery_remains_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);
        using var wsC = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsC, "{\"type\":\"hello\",\"room\":\"room-c\",\"pubkey\":\"peer-c\",\"frontier\":[]}");

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close-a", CancellationToken.None),
            wsB.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close-b", CancellationToken.None),
            SendBinaryFragmentsAsync(wsC, [new byte[] { 0x01 }, new byte[] { 0x02 }])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeA.CloseStatus);

        var closeB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Close, closeB.MessageType);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, closeB.CloseStatus);

        var errorC = await ReceiveTextAsync(wsC);
        Assert.Contains("\"msg\":\"text messages required\"", errorC);

        await SendTextAsync(wsC, "{\"type\":\"noop\"}");
        var ackC = await ReceiveTextAsync(wsC);
        Assert.Contains("\"type\":\"noop-ack\"", ackC);
    }

    [Fact]
    public async Task Runtime_endpoint_empty_text_frame_returns_invalid_json_and_recovers()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, string.Empty);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"invalid JSON\"", error);

        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_empty_binary_frame_returns_text_required_and_recovers()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await ws.SendAsync(Array.Empty<byte>(), WebSocketMessageType.Binary, true, CancellationToken.None);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"text messages required\"", error);

        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_fragmented_binary_frame_returns_text_required_and_recovers()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendBinaryFragmentsAsync(ws, [new byte[] { 0x01 }, new byte[] { 0x02 }]);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"text messages required\"", error);

        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_fragmented_hello_and_noop_emit_noop_ack()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextFragmentsAsync(ws, [
            "{\"type\":\"hello\",\"room\":\"room-a\",",
            "\"pubkey\":\"peer-a\",\"frontier\":[]}" ]);
        await SendTextFragmentsAsync(ws, ["{\"type\":\"no", "op\"}"]);

        var ack = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", ack);
    }

    [Fact]
    public async Task Runtime_endpoint_same_room_pack_is_relayed_to_other_peer()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-sync\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-sync\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        // The peer-joined broadcast can race with subsequent traffic in this harness.
        // Probe briefly, but do not fail this test on missing presence frame; the core
        // contract here is pack relay within the same room.
        for (var attempt = 0; attempt < 4; attempt++)
        {
            var frame = await TryReceiveTextAsync(wsA, TimeSpan.FromMilliseconds(500));
            if (frame is not null
                && frame.Contains("\"type\":\"peer-joined\"", StringComparison.Ordinal)
                && frame.Contains("\"from\":\"peer-b\"", StringComparison.Ordinal))
            {
                break;
            }
        }

        await SendTextAsync(wsA, "{\"type\":\"pack\",\"nodes\":\"AQI=\"}");

        string? relayedPack = null;
        for (var attempt = 0; attempt < 4; attempt++)
        {
            var frame = await TryReceiveTextAsync(wsB, TimeSpan.FromMilliseconds(500));
            if (frame is not null && frame.Contains("\"type\":\"pack\"", StringComparison.Ordinal))
            {
                relayedPack = frame;
                break;
            }
        }

        Assert.NotNull(relayedPack);
        Assert.Contains("\"type\":\"pack\"", relayedPack!);
        Assert.Contains("\"from\":\"peer-a\"", relayedPack!);
        Assert.Contains("\"nodes\":\"AQI=\"", relayedPack!);
    }

    [Fact]
    public async Task Runtime_endpoint_inbound_pack_notifies_registered_observer()
    {
        // S7.1: end-to-end proof of the DI/overload mechanism — an IInboundPackObserver
        // registered the same way Studio's replication sink follow-up will (services.AddSingleton
        // as an additional collaborator, no changes to AddNodalMergeRuntimeCore or the endpoint
        // mapping) is picked up automatically by RuntimeWebSocketLoopRunner's new constructor
        // overload and fires for a real inbound "pack" frame over an actual WebSocket connection.
        var observer = new RecordingInboundPackObserver();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
            services.AddSingleton<IInboundPackObserver>(observer);
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);
        await SendTextAsync(ws, "{\"type\":\"hello\",\"room\":\"room-observer\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(ws, "{\"type\":\"pack\",\"nodes\":\"AQI=\"}");

        for (var attempt = 0; attempt < 20 && observer.Calls.Count == 0; attempt++)
        {
            await Task.Delay(100);
        }

        var call = Assert.Single(observer.Calls);
        Assert.Equal("room-observer", call.RoomId);
        Assert.Equal("AQI=", call.NodesB64);
    }

    [Fact]
    public async Task Runtime_endpoint_same_room_disconnect_emits_peer_left()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-presence\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-presence\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        await SendTextAsync(wsB, "{\"type\":\"noop\"}");
        var noopAck = await ReceiveUntilContainsAsync(wsB, "\"type\":\"noop-ack\"", 4, TimeSpan.FromMilliseconds(500));
        Assert.NotNull(noopAck);

        await wsA.CloseAsync(WebSocketCloseStatus.NormalClosure, "test-disconnect", CancellationToken.None);

        string? peerLeft = null;
        for (var attempt = 0; attempt < 4; attempt++)
        {
            var frame = await TryReceiveTextAsync(wsB, TimeSpan.FromMilliseconds(500));
            if (frame is not null && frame.Contains("\"type\":\"peer-left\"", StringComparison.Ordinal))
            {
                peerLeft = frame;
                break;
            }
        }

        Assert.NotNull(peerLeft);
        Assert.Contains("\"type\":\"peer-left\"", peerLeft!);
        Assert.Contains("\"from\":\"peer-a\"", peerLeft!);
    }

    [Fact]
    public async Task Runtime_endpoint_pack_relay_does_not_cross_rooms()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectRuntimeWebSocketAsync(app);
        using var wsB = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(wsA, "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}");
        await SendTextAsync(wsB, "{\"type\":\"hello\",\"room\":\"room-b\",\"pubkey\":\"peer-b\",\"frontier\":[]}");

        await SendTextAsync(wsA, "{\"type\":\"pack\",\"nodes\":\"AQI=\"}");

        var maybeFrame = await TryReceiveTextAsync(wsB, TimeSpan.FromMilliseconds(250));
        Assert.Null(maybeFrame);
    }

    [Fact]
    public async Task Runtime_endpoint_hello_with_invalid_token_returns_error_and_does_not_initialize()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Invalid("expired"))
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"
        );

        var tokenError = await ReceiveTextAsync(ws);
        Assert.Contains("token rejected: expired", tokenError, StringComparison.Ordinal);

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var noopError = await ReceiveTextAsync(ws);
        Assert.Contains("hello must be sent first", noopError, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Runtime_endpoint_hello_with_valid_token_still_dispatches_noop()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new NoopAckRuntimeCommandBridge());
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"noop\"}");
        var outbound = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"noop-ack\"", outbound, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Runtime_endpoint_set_policy_without_policy_admin_capability_is_denied_before_bridge_dispatch()
    {
        var bridge = new RecordingRuntimeCommandBridge();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(bridge);
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:world/**\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"set-policy\",\"default\":\"allow\",\"rules\":[]}");
        var deny = await ReceiveTextAsync(ws);

        Assert.Contains("reject.control_plane_forbidden: command=set-policy requires=policy.admin", deny, StringComparison.Ordinal);
        Assert.DoesNotContain(bridge.GetCommandsSnapshot(), command => command.Contains("SetPolicy", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Runtime_endpoint_set_policy_with_policy_admin_capability_is_allowed()
    {
        var bridge = new RecordingRuntimeCommandBridge();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(bridge);
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"policy.admin\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"set-policy\",\"default\":\"allow\",\"rules\":[]}");

        await WaitForConditionAsync(
            () => bridge.GetCommandsSnapshot().Any(command => command.Contains("SetPolicy", StringComparison.Ordinal)),
            TimeSpan.FromMilliseconds(500)
        );

        Assert.Contains(bridge.GetCommandsSnapshot(), command => command.Contains("SetPolicy", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Runtime_endpoint_start_tick_without_tick_admin_capability_is_denied_before_bridge_dispatch()
    {
        var bridge = new RecordingRuntimeCommandBridge();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(bridge);
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:world/**\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"start-tick\",\"interval_ms\":16,\"intent_prefix\":\"intent/\"}");
        var deny = await ReceiveTextAsync(ws);

        Assert.Contains("reject.control_plane_forbidden: command=start-tick requires=tick.admin", deny, StringComparison.Ordinal);
        Assert.DoesNotContain(bridge.GetCommandsSnapshot(), command => command.Contains("StartTick", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Runtime_endpoint_start_tick_with_tick_admin_capability_is_allowed()
    {
        var bridge = new RecordingRuntimeCommandBridge();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(bridge);
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"tick.admin\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"start-tick\",\"interval_ms\":16,\"intent_prefix\":\"intent/\"}");

        await WaitForConditionAsync(
            () => bridge.GetCommandsSnapshot().Any(command => command.Contains("StartTick", StringComparison.Ordinal)),
            TimeSpan.FromMilliseconds(500)
        );

        Assert.Contains(bridge.GetCommandsSnapshot(), command => command.Contains("StartTick", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Runtime_endpoint_stop_tick_without_tick_admin_capability_is_denied_before_bridge_dispatch()
    {
        var bridge = new RecordingRuntimeCommandBridge();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(bridge);
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:world/**\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"stop-tick\"}");
        var deny = await ReceiveTextAsync(ws);

        Assert.Contains("reject.control_plane_forbidden: command=stop-tick requires=tick.admin", deny, StringComparison.Ordinal);
        Assert.DoesNotContain(bridge.GetCommandsSnapshot(), command => command.Contains("StopTick", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Runtime_endpoint_stop_tick_with_tick_admin_capability_is_allowed()
    {
        var bridge = new RecordingRuntimeCommandBridge();
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(bridge);
            services.AddSingleton<IRoomTokenAuthProvider>(
                new RuntimeTokenAuthProviderStub(RoomTokenValidationResult.Valid)
            );
        });
        await app.StartAsync();

        using var ws = await ConnectRuntimeWebSocketAsync(app);

        await SendTextAsync(
            ws,
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"tick.admin\"],\"sig\":\"beef\"}}"
        );

        await SendTextAsync(ws, "{\"type\":\"stop-tick\"}");

        await WaitForConditionAsync(
            () => bridge.GetCommandsSnapshot().Any(command => command.Contains("StopTick", StringComparison.Ordinal)),
            TimeSpan.FromMilliseconds(500)
        );

        Assert.Contains(bridge.GetCommandsSnapshot(), command => command.Contains("StopTick", StringComparison.Ordinal));
    }

    private static async Task WaitForConditionAsync(Func<bool> condition, TimeSpan timeout)
    {
        var deadline = DateTime.UtcNow + timeout;
        while (DateTime.UtcNow < deadline)
        {
            if (condition())
            {
                return;
            }

            await Task.Delay(10);
        }
    }

    private static WebApplication BuildTestApp(Action<IServiceCollection>? configureServices = null)
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }

    private static async Task<WebSocket> ConnectRuntimeWebSocketAsync(WebApplication app, string path = "/ws/runtime")
    {
        var client = app.GetTestServer().CreateWebSocketClient();
        return await client.ConnectAsync(new Uri($"ws://localhost{path}"), CancellationToken.None);
    }

    private static async Task SendTextAsync(WebSocket ws, string text)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        await ws.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
    }

    private static async Task SendTextFragmentsAsync(WebSocket ws, IReadOnlyList<string> fragments)
    {
        for (var i = 0; i < fragments.Count; i++)
        {
            var bytes = Encoding.UTF8.GetBytes(fragments[i]);
            var endOfMessage = i == fragments.Count - 1;
            await ws.SendAsync(bytes, WebSocketMessageType.Text, endOfMessage, CancellationToken.None);
        }
    }

    private static async Task SendBinaryFragmentsAsync(WebSocket ws, IReadOnlyList<byte[]> fragments)
    {
        for (var i = 0; i < fragments.Count; i++)
        {
            var endOfMessage = i == fragments.Count - 1;
            await ws.SendAsync(fragments[i], WebSocketMessageType.Binary, endOfMessage, CancellationToken.None);
        }
    }

    private static async Task<string> ReceiveTextAsync(WebSocket ws)
    {
        var frame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Text, frame.MessageType);
        return frame.Text;
    }

    private static async Task<string?> TryReceiveTextAsync(WebSocket ws, TimeSpan timeout)
    {
        var frame = await TryReceiveFrameAsync(ws, timeout);
        if (frame is null)
        {
            return null;
        }

        Assert.Equal(WebSocketMessageType.Text, frame.MessageType);
        return frame.Text;
    }

    private static async Task<string?> ReceiveUntilContainsAsync(
        WebSocket ws,
        string contains,
        int attempts,
        TimeSpan perAttemptTimeout)
    {
        for (var attempt = 0; attempt < attempts; attempt++)
        {
            var frame = await TryReceiveTextAsync(ws, perAttemptTimeout);
            if (frame is not null && frame.Contains(contains, StringComparison.Ordinal))
            {
                return frame;
            }
        }

        return null;
    }

    private static async Task<WebSocketFrame> ReceiveFrameAsync(WebSocket ws)
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

        return result.MessageType == WebSocketMessageType.Text
            ? new WebSocketFrame(result.MessageType, Encoding.UTF8.GetString(ms.ToArray()), result.CloseStatus)
            : new WebSocketFrame(result.MessageType, string.Empty, result.CloseStatus);
    }

    private static async Task<WebSocketFrame?> TryReceiveFrameAsync(WebSocket ws, TimeSpan timeout)
    {
        using var cts = new CancellationTokenSource(timeout);
        var buffer = new byte[8 * 1024];
        using var ms = new MemoryStream();

        try
        {
            WebSocketReceiveResult result;
            do
            {
                result = await ws.ReceiveAsync(buffer, cts.Token);
                if (result.Count > 0)
                {
                    await ms.WriteAsync(buffer.AsMemory(0, result.Count), cts.Token);
                }
            }
            while (!result.EndOfMessage);

            return result.MessageType == WebSocketMessageType.Text
                ? new WebSocketFrame(result.MessageType, Encoding.UTF8.GetString(ms.ToArray()), result.CloseStatus)
                : new WebSocketFrame(result.MessageType, string.Empty, result.CloseStatus);
        }
        catch (OperationCanceledException)
        {
            return null;
        }
    }

    private sealed record WebSocketFrame(
        WebSocketMessageType MessageType,
        string Text,
        WebSocketCloseStatus? CloseStatus
    );
}

internal sealed class NoopAckRuntimeCommandBridge : IRuntimeCommandBridge
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

        if (commandNode.GetValueKind() == System.Text.Json.JsonValueKind.String
            && string.Equals(commandNode.GetValue<string>(), "Noop", StringComparison.Ordinal))
        {
            return FfiJsonBridgeResult.Success("[\"NoopAck\"]");
        }

        return FfiJsonBridgeResult.Success("[]");
    }
}

internal sealed class ParallelRuntimeCommandBridge : IRuntimeCommandBridge
{
    private int _roomBNoopFailures;

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

        if (commandNode.GetValueKind() == System.Text.Json.JsonValueKind.String
            && string.Equals(commandNode.GetValue<string>(), "Noop", StringComparison.Ordinal))
        {
            var roomId = root?["room_id"]?.GetValue<string>();
            if (string.Equals(roomId, "room-b", StringComparison.Ordinal)
                && Interlocked.Increment(ref _roomBNoopFailures) == 1)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.Protocol);
            }

            return FfiJsonBridgeResult.Success("[\"NoopAck\"]");
        }

        return FfiJsonBridgeResult.Success("[]");
    }
}

internal sealed class AlternatingRuntimeCommandBridge : IRuntimeCommandBridge
{
    private int _noopCount;

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

        if (commandNode.GetValueKind() == System.Text.Json.JsonValueKind.String
            && string.Equals(commandNode.GetValue<string>(), "Noop", StringComparison.Ordinal))
        {
            if (Interlocked.Increment(ref _noopCount) % 2 == 1)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.Protocol);
            }

            return FfiJsonBridgeResult.Success("[\"NoopAck\"]");
        }

        return FfiJsonBridgeResult.Success("[]");
    }
}

internal sealed class RuntimeTokenAuthProviderStub : IRoomTokenAuthProvider
{
    private readonly RoomTokenValidationResult _validationResult;

    public RuntimeTokenAuthProviderStub(RoomTokenValidationResult validationResult)
    {
        _validationResult = validationResult;
    }

    public ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(_validationResult);
    }

    public ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(RoomTokenMintResult.NotSupported);
    }
}


internal sealed class RecordingRuntimeCommandBridge : IRuntimeCommandBridge
{
    private readonly object _gate = new();

    public List<string> Commands { get; } = [];

    public IReadOnlyList<string> GetCommandsSnapshot()
    {
        lock (_gate)
        {
            return Commands.ToArray();
        }
    }

    public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
    {
        lock (_gate)
        {
            Commands.Add(commandJson);
        }

        return FfiJsonBridgeResult.Success("[]");
    }
}