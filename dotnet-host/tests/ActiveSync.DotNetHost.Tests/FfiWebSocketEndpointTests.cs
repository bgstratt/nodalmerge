using ActiveSync.DotNetHost;
using ActiveSync.DotNetHost.Ffi;
using ActiveSync.DotNetHost.Runtime;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using System.Net.WebSockets;
using System.Text;

namespace ActiveSync.DotNetHost.Tests;

public class FfiWebSocketEndpointTests
{
    [Fact]
    public async Task Ffi_endpoint_non_websocket_request_returns_400()
    {
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/ws/ffi");

        Assert.Equal(System.Net.HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task Ffi_endpoint_text_message_returns_binary_required_error()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendTextAsync(ws, "not-binary");

        var outbound = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"error\"", outbound);
        Assert.Contains("\"msg\":\"binary messages required\"", outbound);

        await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "test done", CancellationToken.None);
    }

    [Fact]
    public async Task Ffi_endpoint_binary_failure_returns_status_error_text()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Failure(AsStatus.Protocol)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendBinaryAsync(ws, [0x01, 0x02]);

        var outbound = await ReceiveTextAsync(ws);
        Assert.Contains("\"status\":\"Protocol\"", outbound);

        await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "test done", CancellationToken.None);
    }

    [Theory]
    [InlineData(AsStatus.InvalidArg)]
    [InlineData(AsStatus.NotFound)]
    [InlineData(AsStatus.Auth)]
    [InlineData(AsStatus.Policy)]
    [InlineData(AsStatus.Protocol)]
    [InlineData(AsStatus.Internal)]
    public async Task Ffi_endpoint_status_matrix_returns_error_and_recovers(AsStatus status)
    {
        var successPayload = new byte[] { 0x44, 0x45 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Failure(status),
                FfiBridgeResult.Success(successPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendBinaryAsync(ws, [0xAA]);

        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"type\":\"error\"", error);
        Assert.Contains($"\"status\":\"{status}\"", error);

        await SendBinaryAsync(ws, [0xBB]);
        var success = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(successPayload, success.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_fragmented_binary_failure_returns_status_error_and_recovers()
    {
        var successPayload = new byte[] { 0x90, 0x91 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Failure(AsStatus.Protocol),
                FfiBridgeResult.Success(successPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendBinaryFragmentsAsync(ws, [new byte[] { 0x01 }, new byte[] { 0x02 }]);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"status\":\"Protocol\"", error);

        await SendBinaryAsync(ws, [0x03]);
        var success = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(successPayload, success.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_binary_success_returns_binary_payload()
    {
        var eventsPayload = new byte[] { 0xAA, 0xBB, 0xCC };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendBinaryAsync(ws, [0x10]);

        var frame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, frame.MessageType);
        Assert.Equal(eventsPayload, frame.Bytes);

        await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "test done", CancellationToken.None);
    }

    [Fact]
    public async Task Ffi_endpoint_remains_open_after_text_error_and_accepts_follow_on_binary()
    {
        var eventsPayload = new byte[] { 0x21, 0x22 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);

        await SendTextAsync(ws, "invalid-frame-type");
        var errorFrame = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"binary messages required\"", errorFrame);

        await SendBinaryAsync(ws, [0x99]);
        var successFrame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, successFrame.MessageType);
        Assert.Equal(eventsPayload, successFrame.Bytes);

        await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "test done", CancellationToken.None);
    }

    [Fact]
    public async Task Ffi_endpoint_accepts_empty_binary_payload()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success([])
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendBinaryAsync(ws, []);

        var frame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, frame.MessageType);
        Assert.Empty(frame.Bytes);

        await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "test done", CancellationToken.None);
    }

    [Fact]
    public async Task Ffi_endpoint_oversized_binary_message_returns_error_and_keeps_connection_open()
    {
        var eventsPayload = new byte[] { 0x31, 0x32 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        var chunkA = new byte[32_000];
        var chunkB = new byte[32_000];
        var chunkC = new byte[1_537];
        await SendBinaryFragmentsAsync(ws, [chunkA, chunkB, chunkC]);

        var sizeError = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"message too large\"", sizeError);

        await SendBinaryAsync(ws, [0xAB]);
        var successFrame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, successFrame.MessageType);
        Assert.Equal(eventsPayload, successFrame.Bytes);

        await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "test done", CancellationToken.None);
    }

    [Fact]
    public async Task Ffi_endpoint_client_abort_during_receive_allows_follow_on_connection()
    {
        var firstPayload = new byte[] { 0x41 };
        var secondPayload = new byte[] { 0x51, 0x52 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(firstPayload),
                FfiBridgeResult.Success(secondPayload)
            ));
        });
        await app.StartAsync();

        using (var ws = await ConnectFfiWebSocketAsync(app))
        {
            await SendBinaryAsync(ws, [0xA1]);
            var frame = await ReceiveFrameAsync(ws);
            Assert.Equal(WebSocketMessageType.Binary, frame.MessageType);
            Assert.Equal(firstPayload, frame.Bytes);
            ws.Abort();
        }

        using var ws2 = await ConnectFfiWebSocketAsync(app);
        await SendBinaryAsync(ws2, [0xB1]);
        var nextFrame = await ReceiveFrameAsync(ws2);
        Assert.Equal(WebSocketMessageType.Binary, nextFrame.MessageType);
        Assert.Equal(secondPayload, nextFrame.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_partial_fragment_then_abort_allows_follow_on_connection()
    {
        var nextPayload = new byte[] { 0x61, 0x62 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(nextPayload)
            ));
        });
        await app.StartAsync();

        using (var ws = await ConnectFfiWebSocketAsync(app))
        {
            await ws.SendAsync(new byte[] { 0xA1 }, WebSocketMessageType.Binary, endOfMessage: false, CancellationToken.None);
            ws.Abort();
        }

        using var ws2 = await ConnectFfiWebSocketAsync(app);
        await SendBinaryAsync(ws2, [0xB1]);
        var success = await ReceiveFrameAsync(ws2);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(nextPayload, success.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_reconnect_soak_binary_submit_remains_stable()
    {
        var bridgeResults = new List<FfiBridgeResult>();
        for (var i = 0; i < 10; i++)
        {
            bridgeResults.Add(FfiBridgeResult.Success([(byte)(0x80 + i)]));
        }

        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge([.. bridgeResults]));
        });
        await app.StartAsync();

        for (var i = 0; i < 10; i++)
        {
            using var ws = await ConnectFfiWebSocketAsync(app);
            await SendBinaryAsync(ws, [(byte)i]);

            var frame = await ReceiveFrameAsync(ws);
            Assert.Equal(WebSocketMessageType.Binary, frame.MessageType);
            Assert.Equal(new byte[] { (byte)(0x80 + i) }, frame.Bytes);

            await ws.CloseAsync(WebSocketCloseStatus.NormalClosure, "soak-iteration-done", CancellationToken.None);
        }
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_connections_mixed_outcomes_are_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new ParallelFfiBinaryBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        await Task.WhenAll(
            SendBinaryAsync(wsA, [0xA0]),
            SendBinaryAsync(wsB, [0xB0])
        );

        var errorA = await ReceiveTextAsync(wsA);
        Assert.Contains("\"type\":\"error\"", errorA);
        Assert.Contains("\"status\":\"Protocol\"", errorA);

        var successB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
        Assert.Equal(new byte[] { 0xB0, 0xEE }, successB.Bytes);

        await SendBinaryAsync(wsA, [0xB1]);
        var recoveryA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Binary, recoveryA.MessageType);
        Assert.Equal(new byte[] { 0xB1, 0xEE }, recoveryA.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_churn_alternating_failures_remain_isolated_and_recover()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new AlternatingFfiBinaryBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        for (var round = 0; round < 4; round++)
        {
            await Task.WhenAll(
                SendBinaryAsync(wsA, [0xA0]),
                SendBinaryAsync(wsB, [0xB0])
            );

            var successA = await ReceiveFrameAsync(wsA);
            Assert.Equal(WebSocketMessageType.Binary, successA.MessageType);
            Assert.Equal(new byte[] { 0xA0, 0xEE }, successA.Bytes);

            if (round % 2 == 0)
            {
                var errorB = await ReceiveTextAsync(wsB);
                Assert.Contains("\"type\":\"error\"", errorB);
                Assert.Contains("\"status\":\"Protocol\"", errorB);

                await SendBinaryAsync(wsB, [0xB1]);
                var recoveryB = await ReceiveFrameAsync(wsB);
                Assert.Equal(WebSocketMessageType.Binary, recoveryB.MessageType);
                Assert.Equal(new byte[] { 0xB1, 0xEE }, recoveryB.Bytes);
            }
            else
            {
                var successB = await ReceiveFrameAsync(wsB);
                Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
                Assert.Equal(new byte[] { 0xB0, 0xEE }, successB.Bytes);
            }
        }
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_fragmented_churn_mixed_frames_remain_isolated_and_recover()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new AlternatingFfiBinaryBridge());
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        var totalErrorCount = 0;
        for (var round = 0; round < 6; round++)
        {
            await Task.WhenAll(
                SendBinaryFragmentsAsync(wsA, [new byte[] { 0xA0 }, new byte[] { 0xA1 }]),
                SendBinaryFragmentsAsync(wsB, [new byte[] { 0xB0 }, new byte[] { 0xB1 }])
            );

            var frameA = await ReceiveFrameAsync(wsA);
            Assert.Equal(WebSocketMessageType.Binary, frameA.MessageType);
            Assert.Equal(new byte[] { 0xA0, 0xEE }, frameA.Bytes);

            var frameB = await ReceiveFrameAsync(wsB);
            if (frameB.MessageType == WebSocketMessageType.Text)
            {
                var error = Encoding.UTF8.GetString(frameB.Bytes);
                Assert.Contains("\"type\":\"error\"", error);
                Assert.Contains("\"status\":\"Protocol\"", error);
                totalErrorCount++;

                var recoveredB = false;
                for (var attempt = 0; attempt < 3; attempt++)
                {
                    await SendBinaryAsync(wsB, [0xB2]);
                    var recovery = await ReceiveFrameAsync(wsB);
                    if (recovery.MessageType == WebSocketMessageType.Binary)
                    {
                        Assert.Equal(new byte[] { 0xB2, 0xEE }, recovery.Bytes);
                        recoveredB = true;
                        break;
                    }

                    var recoveryError = Encoding.UTF8.GetString(recovery.Bytes);
                    Assert.Contains("\"type\":\"error\"", recoveryError);
                    Assert.Contains("\"status\":\"Protocol\"", recoveryError);
                    totalErrorCount++;
                }

                Assert.True(recoveredB, "expected socket B to recover with binary response after protocol failures");
            }
            else
            {
                Assert.Equal(WebSocketMessageType.Binary, frameB.MessageType);
                Assert.Equal(new byte[] { 0xB0, 0xEE }, frameB.Bytes);
            }
        }

        Assert.True(totalErrorCount >= 3, "expected repeated protocol failures during fragmented churn rounds");
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_fragmented_oversized_interleaving_remains_isolated_and_recovers()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0xAA, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0xAB, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0xAC, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0xAD, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0xAE, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0xAF, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        var chunkA = new byte[32_000];
        var chunkB = new byte[32_000];
        var chunkC = new byte[1_537];

        for (var round = 0; round < 3; round++)
        {
            await Task.WhenAll(
                SendBinaryFragmentsAsync(wsA, [chunkA, chunkB, chunkC]),
                SendBinaryFragmentsAsync(wsB, [new byte[] { 0x11 }, new byte[] { 0x22 }])
            );

            var errorA = await ReceiveTextAsync(wsA);
            Assert.Contains("\"msg\":\"message too large\"", errorA);

            var successB = await ReceiveFrameAsync(wsB);
            Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
            Assert.Equal(new byte[] { (byte)(0xAA + (round * 2)), 0xEE }, successB.Bytes);

            await SendBinaryAsync(wsA, [0x31]);
            var recoveryA = await ReceiveFrameAsync(wsA);
            Assert.Equal(WebSocketMessageType.Binary, recoveryA.MessageType);
            Assert.Equal(new byte[] { (byte)(0xAB + (round * 2)), 0xEE }, recoveryA.Bytes);
        }
    }

    [Fact]
    public async Task Ffi_endpoint_client_close_output_triggers_normal_server_close()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "client half-close", CancellationToken.None);

        var frame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Close, frame.MessageType);
    }

    [Fact]
    public async Task Ffi_endpoint_partial_fragment_then_client_close_output_closes_cleanly()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await ws.SendAsync(new byte[] { 0xCA, 0xFE }, WebSocketMessageType.Binary, endOfMessage: false, CancellationToken.None);
        await ws.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "client half-close after fragment", CancellationToken.None);

        var frame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Close, frame.MessageType);
    }

    [Fact]
    public async Task Ffi_endpoint_client_close_output_with_non_normal_status_still_closes_normally()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge());
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await ws.CloseOutputAsync(WebSocketCloseStatus.PolicyViolation, "policy-close", CancellationToken.None);

        var frame = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Close, frame.MessageType);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_half_close_and_active_binary_traffic_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0x41, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0x42, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendBinaryAsync(wsB, [0xAA])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);

        var successB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
        Assert.Equal(new byte[] { 0x41, 0xEE }, successB.Bytes);

        await SendBinaryAsync(wsB, [0xAB]);
        var successB2 = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB2.MessageType);
        Assert.Equal(new byte[] { 0x42, 0xEE }, successB2.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_half_close_and_active_error_recovery_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0x51, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendTextAsync(wsB, "bad-frame")
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);

        var errorB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"msg\":\"binary messages required\"", errorB);

        await SendBinaryAsync(wsB, [0x01]);
        var successB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
        Assert.Equal(new byte[] { 0x51, 0xEE }, successB.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_half_close_and_oversized_recovery_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0x61, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        var chunkA = new byte[32_000];
        var chunkB = new byte[32_000];
        var chunkC = new byte[1_537];

        await Task.WhenAll(
            SendBinaryFragmentsAsync(wsA, [chunkA, chunkB, chunkC]),
            wsB.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None)
        );

        var errorA = await ReceiveTextAsync(wsA);
        Assert.Contains("\"msg\":\"message too large\"", errorA);

        await SendBinaryAsync(wsA, [0x22]);
        var successA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Binary, successA.MessageType);
        Assert.Equal(new byte[] { 0x61, 0xEE }, successA.Bytes);

        var closeB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Close, closeB.MessageType);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_half_close_and_fragmented_active_success_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0x71, 0xEE }),
                FfiBridgeResult.Success(new byte[] { 0x72, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendBinaryFragmentsAsync(wsB, [new byte[] { 0x01 }, new byte[] { 0x02 }])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);

        var successB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
        Assert.Equal(new byte[] { 0x71, 0xEE }, successB.Bytes);

        await SendBinaryAsync(wsB, [0x03]);
        var successB2 = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB2.MessageType);
        Assert.Equal(new byte[] { 0x72, 0xEE }, successB2.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_half_close_and_fragmented_active_error_recovery_remain_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0x81, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close", CancellationToken.None),
            SendTextFragmentsAsync(wsB, ["ba", "d"])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);

        var errorB = await ReceiveTextAsync(wsB);
        Assert.Contains("\"msg\":\"binary messages required\"", errorB);

        await SendBinaryAsync(wsB, [0x04]);
        var successB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Binary, successB.MessageType);
        Assert.Equal(new byte[] { 0x81, 0xEE }, successB.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_parallel_dual_half_close_with_third_active_recovery_remains_isolated()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(new byte[] { 0x91, 0xEE })
            ));
        });
        await app.StartAsync();

        using var wsA = await ConnectFfiWebSocketAsync(app);
        using var wsB = await ConnectFfiWebSocketAsync(app);
        using var wsC = await ConnectFfiWebSocketAsync(app);

        await Task.WhenAll(
            wsA.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close-a", CancellationToken.None),
            wsB.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "parallel-half-close-b", CancellationToken.None),
            SendTextFragmentsAsync(wsC, ["ba", "d"])
        );

        var closeA = await ReceiveFrameAsync(wsA);
        Assert.Equal(WebSocketMessageType.Close, closeA.MessageType);

        var closeB = await ReceiveFrameAsync(wsB);
        Assert.Equal(WebSocketMessageType.Close, closeB.MessageType);

        var errorC = await ReceiveTextAsync(wsC);
        Assert.Contains("\"msg\":\"binary messages required\"", errorC);

        await SendBinaryAsync(wsC, [0x05]);
        var successC = await ReceiveFrameAsync(wsC);
        Assert.Equal(WebSocketMessageType.Binary, successC.MessageType);
        Assert.Equal(new byte[] { 0x91, 0xEE }, successC.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_empty_text_frame_returns_binary_required_and_recovers()
    {
        var eventsPayload = new byte[] { 0x77 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendTextAsync(ws, string.Empty);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"binary messages required\"", error);

        await SendBinaryAsync(ws, [0x01]);
        var success = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(eventsPayload, success.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_fragmented_text_frame_returns_binary_required_and_recovers()
    {
        var eventsPayload = new byte[] { 0x66, 0x67 };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendTextFragmentsAsync(ws, ["in", "valid"]);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"binary messages required\"", error);

        await SendBinaryAsync(ws, [0x42]);
        var success = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(eventsPayload, success.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_fragmented_text_error_then_fragmented_binary_success_recovers()
    {
        var eventsPayload = new byte[] { 0x7A, 0x7B, 0x7C };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendTextFragmentsAsync(ws, ["ba", "d"]);
        var error = await ReceiveTextAsync(ws);
        Assert.Contains("\"msg\":\"binary messages required\"", error);

        await SendBinaryFragmentsAsync(ws, [new byte[] { 0x11 }, new byte[] { 0x22 }]);
        var success = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(eventsPayload, success.Bytes);
    }

    [Fact]
    public async Task Ffi_endpoint_fragmented_binary_message_success_returns_binary_payload()
    {
        var eventsPayload = new byte[] { 0xDE, 0xAD, 0xBE, 0xEF };
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(eventsPayload)
            ));
        });
        await app.StartAsync();

        using var ws = await ConnectFfiWebSocketAsync(app);
        await SendBinaryFragmentsAsync(ws, [new byte[] { 0xA0 }, new byte[] { 0xB0, 0xC0 }]);

        var success = await ReceiveFrameAsync(ws);
        Assert.Equal(WebSocketMessageType.Binary, success.MessageType);
        Assert.Equal(eventsPayload, success.Bytes);
    }

    private static WebApplication BuildTestApp(Action<IServiceCollection>? configureServices = null)
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }

    private static async Task<WebSocket> ConnectFfiWebSocketAsync(WebApplication app)
    {
        var client = app.GetTestServer().CreateWebSocketClient();
        return await client.ConnectAsync(new Uri("ws://localhost/ws/ffi"), CancellationToken.None);
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

    private static async Task SendBinaryAsync(WebSocket ws, byte[] payload)
    {
        await ws.SendAsync(payload, WebSocketMessageType.Binary, true, CancellationToken.None);
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

    private static async Task<TestWebSocketFrame> ReceiveFrameAsync(WebSocket ws)
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

        var bytes = ms.ToArray();
        return new TestWebSocketFrame(
            result.MessageType,
            Encoding.UTF8.GetString(bytes),
            bytes
        );
    }

    private sealed record TestWebSocketFrame(
        WebSocketMessageType MessageType,
        string Text,
        byte[] Bytes
    );
}

internal sealed class FakeFfiBinaryBridge : IFfiBinaryBridge
{
    private readonly Queue<FfiBridgeResult> _results;

    public FakeFfiBinaryBridge(params FfiBridgeResult[] results)
    {
        _results = new Queue<FfiBridgeResult>(results);
    }

    public FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload)
    {
        if (_results.Count == 0)
        {
            return FfiBridgeResult.Success([]);
        }

        return _results.Dequeue();
    }
}

internal sealed class ParallelFfiBinaryBridge : IFfiBinaryBridge
{
    public FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload)
    {
        if (commandPayload.Length > 0 && commandPayload[0] == 0xA0)
        {
            return FfiBridgeResult.Failure(AsStatus.Protocol);
        }

        var tag = commandPayload.Length > 0 ? commandPayload[0] : (byte)0x00;
        return FfiBridgeResult.Success([tag, 0xEE]);
    }
}

internal sealed class AlternatingFfiBinaryBridge : IFfiBinaryBridge
{
    private int _b0Calls;

    public FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload)
    {
        if (commandPayload.Length > 0 && commandPayload[0] == 0xB0
            && Interlocked.Increment(ref _b0Calls) % 2 == 1)
        {
            return FfiBridgeResult.Failure(AsStatus.Protocol);
        }

        var tag = commandPayload.Length > 0 ? commandPayload[0] : (byte)0x00;
        return FfiBridgeResult.Success([tag, 0xEE]);
    }
}