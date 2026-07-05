using NodalMerge.DotNetHost;
using NodalMerge.DotNetHost.Ffi;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public class FfiHttpEndpointTests
{
    [Fact]
    public async Task Abi_version_endpoint_returns_bridge_version()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiHttpBridge>(new FakeFfiHttpBridge(abiVersion: 77));
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/ffi/abi-version");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal(77u, doc.RootElement.GetProperty("abiVersion").GetUInt32());
    }

    [Fact]
    public async Task Submit_endpoint_returns_bad_request_with_status_on_bridge_failure()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiHttpBridge>(new FakeFfiHttpBridge(
                abiVersion: 1,
                submitResult: (AsStatus.Protocol, [])
            ));
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        using var content = new ByteArrayContent([0x01, 0x02]);
        content.Headers.ContentType = new MediaTypeHeaderValue("application/octet-stream");
        var response = await client.PostAsync("/ffi/submit", content);

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal("Protocol", doc.RootElement.GetProperty("status").GetString());
        Assert.Equal(0, doc.RootElement.GetProperty("eventsLength").GetInt32());
    }

    [Fact]
    public async Task Submit_endpoint_returns_binary_payload_on_success()
    {
        var eventsPayload = Encoding.UTF8.GetBytes("events");
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiHttpBridge>(new FakeFfiHttpBridge(
                abiVersion: 1,
                submitResult: (AsStatus.Ok, eventsPayload)
            ));
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        using var content = new ByteArrayContent([0xAA]);
        content.Headers.ContentType = new MediaTypeHeaderValue("application/octet-stream");
        var response = await client.PostAsync("/ffi/submit", content);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Equal("application/octet-stream", response.Content.Headers.ContentType?.MediaType);
        var payload = await response.Content.ReadAsByteArrayAsync();
        Assert.Equal(eventsPayload, payload);
    }

    [Fact]
    public async Task Submit_endpoint_forwards_empty_body_to_bridge()
    {
        var bridge = new FakeFfiHttpBridge(
            abiVersion: 1,
            submitResult: (AsStatus.Ok, [])
        );

        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IFfiHttpBridge>(bridge);
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        using var content = new ByteArrayContent([]);
        var response = await client.PostAsync("/ffi/submit", content);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Equal(1, bridge.SubmitCallCount);
        Assert.NotNull(bridge.LastSubmittedPayload);
        Assert.Empty(bridge.LastSubmittedPayload!);
    }

    private static WebApplication BuildTestApp(Action<IServiceCollection>? configureServices = null)
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }
}

internal sealed class FakeFfiHttpBridge : IFfiHttpBridge
{
    private readonly uint _abiVersion;
    private readonly (AsStatus status, byte[] eventsPayload) _submitResult;

    public int SubmitCallCount { get; private set; }
    public byte[]? LastSubmittedPayload { get; private set; }

    public FakeFfiHttpBridge(uint abiVersion, (AsStatus status, byte[] eventsPayload)? submitResult = null)
    {
        _abiVersion = abiVersion;
        _submitResult = submitResult ?? (AsStatus.Ok, []);
    }

    public uint GetAbiVersion()
    {
        return _abiVersion;
    }

    public (AsStatus status, byte[] eventsPayload) SubmitCommand(byte[] commandPayload)
    {
        SubmitCallCount += 1;
        LastSubmittedPayload = commandPayload;
        return _submitResult;
    }
}