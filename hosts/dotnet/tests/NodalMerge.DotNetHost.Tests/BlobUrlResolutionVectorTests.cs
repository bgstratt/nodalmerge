using System.Text.Json;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.DependencyInjection.Extensions;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Drives <c>GET /blobs/{hash}/url</c> and <c>POST /blobs/{hash}/uploaded</c>
/// (docs/BLOB_HTTP_SURFACE.md "Blob URL resolution (optional capability)",
/// slice S4.1) against the frozen golden vectors
/// (<c>engine/commands/blob-url-resolution-vectors.v1.json</c>). Unlike
/// <see cref="BlobHttpSurfaceTests"/>, expected behavior here hinges on
/// whether a presign-capable backend is registered, not on stored bytes —
/// each vector's <c>backend_configured</c> flag selects between a
/// <see cref="FakeBlobUrlResolverProvider"/> that returns the vectors'
/// <c>fake_presigned_url</c> and no resolver at all. The Rust server has no
/// HTTP implementation of this section yet (slice 4.2); this file exists so
/// that harness can consume the same vectors without a parallel
/// vector-authoring pass.
/// </summary>
public sealed class BlobUrlResolutionVectorTests
{
    private sealed record UrlVector(
        string Id,
        string Route,
        string Hash,
        string? Op,
        long? Size,
        string? ContentType,
        bool BackendConfigured,
        int ExpectStatus,
        IReadOnlyList<string>? ExpectResponseShape,
        string? ExpectError
    );

    private static readonly string CanonicalHash;
    private static readonly string FakePresignedUrl;
    private static readonly int FakeTtlSeconds;
    private static readonly IReadOnlyList<UrlVector> Vectors;

    static BlobUrlResolutionVectorTests()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "blob-url-resolution-vectors.v1.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var root = doc.RootElement;

        CanonicalHash = root.GetProperty("canonical_hash").GetString()!;
        FakePresignedUrl = root.GetProperty("fake_presigned_url").GetString()!;
        FakeTtlSeconds = root.GetProperty("fake_ttl_seconds").GetInt32();

        var vectors = new List<UrlVector>();
        foreach (var el in root.GetProperty("vectors").EnumerateArray())
        {
            IReadOnlyList<string>? shape = null;
            if (el.TryGetProperty("expect_response_shape", out var shapeEl))
            {
                shape = shapeEl.EnumerateArray().Select(e => e.GetString()!).ToList();
            }

            vectors.Add(new UrlVector(
                el.GetProperty("id").GetString()!,
                el.GetProperty("route").GetString()!,
                el.GetProperty("hash").GetString()!,
                el.TryGetProperty("op", out var opEl) ? opEl.GetString() : null,
                el.TryGetProperty("size", out var sizeEl) ? sizeEl.GetInt64() : null,
                el.TryGetProperty("content_type", out var ctEl) ? ctEl.GetString() : null,
                el.GetProperty("backend_configured").GetBoolean(),
                el.GetProperty("expect_status").GetInt32(),
                shape,
                el.TryGetProperty("expect_error", out var errEl) ? errEl.GetString() : null
            ));
        }
        Vectors = vectors;
    }

    public static TheoryData<string> VectorIds()
    {
        var data = new TheoryData<string>();
        foreach (var v in Vectors)
        {
            data.Add(v.Id);
        }
        return data;
    }

    [Theory]
    [MemberData(nameof(VectorIds))]
    public async Task Vector_matches_contract(string id)
    {
        var vector = Vectors.Single(v => v.Id == id);

        await using var app = BuildTestApp(services =>
        {
            if (vector.BackendConfigured)
            {
                services.AddSingleton<IBlobUrlResolverProvider>(
                    new FakeBlobUrlResolverProvider(
                        putResult: new PresignedBlobUrl(
                            FakePresignedUrl,
                            DateTimeOffset.UtcNow.AddSeconds(FakeTtlSeconds)
                        ),
                        getResult: new PresignedBlobUrl(
                            FakePresignedUrl,
                            DateTimeOffset.UtcNow.AddSeconds(FakeTtlSeconds)
                        )
                    )
                );
            }
            else
            {
                // Model "no delegated backend" by removing the resolver
                // entirely rather than swapping in a null-returning stub —
                // both collapse to 501 per docs/BLOB_HTTP_SURFACE.md, and
                // this also exercises the "no resolver registered at all"
                // branch the handlers check via HttpContext.RequestServices.
                services.RemoveAll<IBlobUrlResolverProvider>();
            }
        });
        await app.StartAsync();
        var client = app.GetTestClient();

        var hash = vector.Hash == "canonical" ? CanonicalHash : "not-a-hash";

        HttpResponseMessage response = vector.Route switch
        {
            "url" => await client.GetAsync($"/blobs/{hash}/url{BuildUrlQuery(vector)}"),
            "uploaded" => await client.PostAsync($"/blobs/{hash}/uploaded", content: null),
            _ => throw new InvalidOperationException($"vector `{vector.Id}`: unknown route `{vector.Route}`")
        };

        Assert.True(
            (int)response.StatusCode == vector.ExpectStatus,
            $"vector `{vector.Id}`: expected status {vector.ExpectStatus}, got {(int)response.StatusCode}"
        );

        if (vector.ExpectResponseShape is null && vector.ExpectError is null)
        {
            return;
        }

        var body = await response.Content.ReadAsStringAsync();
        using var responseDoc = JsonDocument.Parse(body);

        if (vector.ExpectResponseShape is not null)
        {
            foreach (var key in vector.ExpectResponseShape)
            {
                Assert.True(
                    responseDoc.RootElement.TryGetProperty(key, out _),
                    $"vector `{vector.Id}`: expected response to carry key `{key}`"
                );
            }
        }

        if (vector.ExpectError is not null)
        {
            Assert.Equal(vector.ExpectError, responseDoc.RootElement.GetProperty("error").GetString());
        }
    }

    private static string BuildUrlQuery(UrlVector vector)
    {
        var query = $"?op={vector.Op}";
        if (vector.Size is { } size)
        {
            query += $"&size={size}";
        }
        if (vector.ContentType is { } contentType)
        {
            query += $"&contentType={Uri.EscapeDataString(contentType)}";
        }
        return query;
    }

    private static WebApplication BuildTestApp(Action<IServiceCollection> configureServices)
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }
}
