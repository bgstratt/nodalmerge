using System.Net;
using System.Text.Json;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Asserts the .NET file store, GC coordinator, and delegate resolver
/// against the canonical blob layout vectors
/// (<c>engine/commands/blob-layout-vectors.v1.json</c>, linked into this
/// project as <c>blob-layout-vectors.v1.json</c>). The Rust mirrors are
/// <c>server/server/tests/blob_layout_vectors.rs</c> (file layout) and
/// <c>server/s3-blobs/src/lib.rs</c>'s
/// <c>blob_layout_vectors_match_s3_key_derivation</c> test (S3 keys). To
/// change the layout or protocol, update the vectors, all harnesses, and
/// <c>docs/BLOB_STORAGE_LAYOUT.md</c> together.
/// </summary>
public sealed class BlobLayoutParityTests
{
    private sealed record PathVector(string Id, string Hash, string? BlobRelativePath, string? TombstoneRelativePath);
    private sealed record NameVector(string Id, string Name, bool Canonical);
    private sealed record DelegateVector(string Id, JsonElement? Request, JsonElement? Response);

    private static readonly IReadOnlyList<PathVector> PathVectors;
    private static readonly IReadOnlyList<NameVector> NameVectors;
    private static readonly IReadOnlyList<DelegateVector> DelegateVectors;

    static BlobLayoutParityTests()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "blob-layout-vectors.v1.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var root = doc.RootElement;

        var pathVectors = new List<PathVector>();
        foreach (var el in root.GetProperty("path_vectors").EnumerateArray())
        {
            pathVectors.Add(new PathVector(
                el.GetProperty("id").GetString()!,
                el.GetProperty("hash").GetString()!,
                el.TryGetProperty("blob_relative_path", out var b) ? b.GetString() : null,
                el.TryGetProperty("tombstone_relative_path", out var t) ? t.GetString() : null
            ));
        }
        PathVectors = pathVectors;

        var nameVectors = new List<NameVector>();
        foreach (var el in root.GetProperty("name_conformance_vectors").EnumerateArray())
        {
            nameVectors.Add(new NameVector(
                el.GetProperty("id").GetString()!,
                el.GetProperty("name").GetString()!,
                el.GetProperty("canonical").GetBoolean()
            ));
        }
        NameVectors = nameVectors;

        var delegateVectors = new List<DelegateVector>();
        foreach (var el in root.GetProperty("delegate_protocol_v1_vectors").EnumerateArray())
        {
            delegateVectors.Add(new DelegateVector(
                el.GetProperty("id").GetString()!,
                el.TryGetProperty("request", out var r) ? r.Clone() : null,
                el.TryGetProperty("response", out var resp) ? resp.Clone() : null
            ));
        }
        DelegateVectors = delegateVectors;
    }

    public static TheoryData<string> FileLayoutVectorIds()
    {
        var data = new TheoryData<string>();
        foreach (var v in PathVectors.Where(v => v.BlobRelativePath is not null))
        {
            data.Add(v.Id);
        }
        return data;
    }

    [Theory]
    [MemberData(nameof(FileLayoutVectorIds))]
    public async Task FileStore_writes_blob_at_canonical_relative_path(string id)
    {
        var vector = PathVectors.Single(v => v.Id == id);
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-blob-layout-parity", Guid.NewGuid().ToString("N"));

        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                ["NodalMerge:Providers:BlobStorage"] = "File",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = root
            })
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        await using var provider = services.BuildServiceProvider();
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        await blobStore.PutBlobAsync(vector.Hash, [1, 2, 3], null, CancellationToken.None);

        var expectedPath = Path.Combine(root, vector.BlobRelativePath!.Replace('/', Path.DirectorySeparatorChar));
        Assert.True(File.Exists(expectedPath), $"vector `{id}`: expected blob at {expectedPath}");
    }

    public static TheoryData<string> NameConformanceVectorIds()
    {
        var data = new TheoryData<string>();
        foreach (var v in NameVectors)
        {
            data.Add(v.Id);
        }
        return data;
    }

    [Theory]
    [MemberData(nameof(NameConformanceVectorIds))]
    public void FileGc_treats_name_as_canonical_iff_vector_says_so(string id)
    {
        var vector = NameVectors.Single(v => v.Id == id);
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-blob-layout-parity", Guid.NewGuid().ToString("N"));
        var blake3Dir = Path.Combine(root, "blake3");
        Directory.CreateDirectory(blake3Dir);
        File.WriteAllText(Path.Combine(blake3Dir, vector.Name), "content");

        var gc = new FileBlobGcCoordinator(root);
        var report = gc.DryRun(Array.Empty<string>(), DateTimeOffset.UtcNow);

        Assert.Equal(
            vector.Canonical ? 1 : 0,
            report.Scanned
        );
    }

    [Fact]
    public async Task Delegate_put_request_matches_protocol_v1_vector()
    {
        var vector = DelegateVectors.Single(v => v.Id == "put-request");
        var expected = vector.Request!.Value;

        var handler = new CapturingHandler();
        var resolver = await BuildResolverAsync(handler);

        await resolver.ResolvePutUrlAsync(new BlobPutUrlRequest(
            RoomId: expected.GetProperty("room").GetString()!,
            Namespace: expected.GetProperty("namespace").GetString()!,
            HashHex: expected.GetProperty("hash").GetString()!,
            SizeBytes: expected.GetProperty("size").GetInt64(),
            ContentType: expected.GetProperty("content_type").GetString()
        ), CancellationToken.None);

        AssertRequestMatchesVector(expected, handler.LastRequestBody);
    }

    [Fact]
    public async Task Delegate_get_request_matches_protocol_v1_vector()
    {
        var vector = DelegateVectors.Single(v => v.Id == "get-request");
        var expected = vector.Request!.Value;

        var handler = new CapturingHandler();
        var resolver = await BuildResolverAsync(handler);

        await resolver.ResolveGetUrlAsync(new BlobGetUrlRequest(
            RoomId: expected.GetProperty("room").GetString()!,
            Namespace: expected.GetProperty("namespace").GetString()!,
            HashHex: expected.GetProperty("hash").GetString()!
        ), CancellationToken.None);

        AssertRequestMatchesVector(expected, handler.LastRequestBody);
    }

    [Fact]
    public void Delegate_response_vector_is_url_only()
    {
        var vector = DelegateVectors.Single(v => v.Id == "response-shape");
        var response = vector.Response!.Value;

        Assert.True(response.TryGetProperty("url", out _));
        // Protocol v1 responses carry no expiry field — see
        // docs/BLOB_STORAGE_LAYOUT.md §7.
        Assert.Single(response.EnumerateObject());
    }

    private static void AssertRequestMatchesVector(JsonElement expected, string? actualBody)
    {
        Assert.NotNull(actualBody);
        using var actualDoc = JsonDocument.Parse(actualBody!);
        var actual = actualDoc.RootElement;

        foreach (var prop in expected.EnumerateObject())
        {
            Assert.True(actual.TryGetProperty(prop.Name, out var actualValue), $"missing field `{prop.Name}`");
            Assert.Equal(prop.Value.ToString(), actualValue.ToString());
        }
    }

    private static async Task<IBlobUrlResolverProvider> BuildResolverAsync(HttpMessageHandler handler)
    {
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.test"
            })
            .Build();

        var services = new ServiceCollection();
        services.AddSingleton<IHttpClientFactory>(new FakeHttpClientFactory(handler));
        services.AddNodalMergeHostProviders(configuration);
        var provider = services.BuildServiceProvider();
        await Task.CompletedTask;
        return provider.GetRequiredService<IBlobUrlResolverProvider>();
    }

    private sealed class FakeHttpClientFactory(HttpMessageHandler handler) : IHttpClientFactory
    {
        private readonly HttpClient _client = new(handler) { BaseAddress = new Uri("https://delegate.test") };

        public HttpClient CreateClient(string name) => _client;
    }

    private sealed class CapturingHandler : HttpMessageHandler
    {
        public string? LastRequestBody { get; private set; }

        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            LastRequestBody = request.Content is null
                ? null
                : await request.Content.ReadAsStringAsync(cancellationToken);

            return new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent("{\"url\":\"https://storage.example.com/presigned\"}")
            };
        }
    }
}
