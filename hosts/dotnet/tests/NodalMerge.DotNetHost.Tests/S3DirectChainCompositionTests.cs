using System.Net;
using System.Text;
using System.Text.Json;
using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 4.3 acceptance coverage at the full DI-composed chain level:
/// <c>local -&gt; server-relay -&gt; s3-direct</c> via
/// <c>NodalMerge:Providers:BlobStorage = ChainedRemote</c> +
/// <c>NodalMerge:Storage:S3Direct:Enabled = true</c>. The fake origin
/// answers the relay's <c>GET /blobs/{hash}</c> with 404 always — mirroring
/// the reference Rust S3 backend (<c>S3BlobStore::get_blob</c> never
/// hydrates bytes into the server process, so its relay GET always 404s,
/// per <c>blob_url_resolution_minio.rs</c>) — so these tests exercise the
/// real fallback: relay misses, s3-direct hits. Together with
/// <c>ChainedBlobStoreProviderHttpIntegrationTests</c> (the two-link case)
/// this proves <see cref="ChainedBlobStoreProvider"/> is the one and only
/// verify gate regardless of how many remote links are configured.
/// </summary>
public sealed class S3DirectChainCompositionTests
{
    private const string BucketUrl = "https://bucket.test/presigned-object";

    [Fact]
    public async Task Cold_local_store_materializes_via_s3_direct_when_relay_misses()
    {
        var bytes = Encoding.UTF8.GetBytes("slice 4.3 acceptance payload, s3-direct fallback");
        var hash = Hasher.Hash(bytes).ToString();

        var origin = new FakeOriginHandler(relayAlways404: true, bucketBytesByHash: new()
        {
            [hash] = (bytes, ContentEncoding: null)
        });
        var bucket = new BucketHandler(origin);

        var localRoot = NewTempRoot("local-cold");
        try
        {
            var chain = BuildChain(localRoot, origin, bucket);

            var result = await chain.TryGetBlobAsync(hash);

            Assert.True(result.Found);
            Assert.Equal(bytes, result.Bytes);
            Assert.True(origin.RelayGetCallCount >= 1, "expected the relay link to be tried (and miss) first");
            Assert.True(origin.UrlResolveCallCount >= 1, "expected s3-direct to resolve a presigned GET URL");
            Assert.True(
                File.Exists(Path.Combine(localRoot, "blake3", hash)),
                "expected the local cache to now hold the blob after materializing via s3-direct"
            );
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    [Fact]
    public async Task Corrupted_bucket_payload_is_rejected_and_never_cached_locally()
    {
        var correctBytes = Encoding.UTF8.GetBytes("this is the real content, 4.3 edition");
        var hash = Hasher.Hash(correctBytes).ToString();
        var wrongBytes = Encoding.UTF8.GetBytes("this is NOT the real content");

        var origin = new FakeOriginHandler(relayAlways404: true, bucketBytesByHash: new()
        {
            // The bucket serves whatever's actually stored there — simulate
            // corruption directly at the bucket, same posture as the
            // existing relay-corruption test (verification is the chain's
            // job, not any one link's).
            [hash] = (wrongBytes, ContentEncoding: null)
        });
        var bucket = new BucketHandler(origin);

        var localRoot = NewTempRoot("local-corrupt");
        try
        {
            var chain = BuildChain(localRoot, origin, bucket);

            var result = await chain.TryGetBlobAsync(hash);

            Assert.False(result.Found);
            Assert.False(
                File.Exists(Path.Combine(localRoot, "blake3", hash)),
                "a corrupted bucket payload must never be written to the local cache"
            );
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    [Fact]
    public async Task Put_fans_out_to_both_relay_and_s3_direct()
    {
        var bytes = Encoding.UTF8.GetBytes("slice 4.3 put fan-out payload");
        var hash = Hasher.Hash(bytes).ToString();

        var origin = new FakeOriginHandler(relayAlways404: false, bucketBytesByHash: new());
        var bucket = new BucketHandler(origin);

        var localRoot = NewTempRoot("local-putfanout");
        try
        {
            var chain = BuildChain(localRoot, origin, bucket);

            await chain.PutBlobAsync(hash, bytes, "text/plain");

            Assert.True(origin.RelayPutCallCount >= 1, "expected the relay link to receive the put");
            Assert.True(origin.UrlResolveCallCount >= 1, "expected s3-direct to resolve a presigned PUT URL");
            Assert.True(bucket.PutCallCount >= 1, "expected the bucket itself to receive the s3-direct upload");
            Assert.True(origin.UploadedConfirmCallCount >= 1, "expected s3-direct to confirm the upload");
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    private static IBlobStoreProvider BuildChain(string localRoot, HttpMessageHandler originHandler, HttpMessageHandler bucketHandler)
    {
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = localRoot,
                ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = "https://origin.test",
                ["NodalMerge:Storage:S3Direct:Enabled"] = "true"
            })
            .Build();

        var services = new ServiceCollection();
        services.AddSingleton<IHttpClientFactory>(new NamedFakeHttpClientFactory(new Dictionary<string, HttpMessageHandler>
        {
            [HttpRemoteBlobStoreProvider.HttpClientName] = originHandler,
            [S3DirectBlobStoreProvider.BucketHttpClientName] = bucketHandler
        }, "https://origin.test"));
        services.AddNodalMergeHostProviders(configuration);

        var provider = services.BuildServiceProvider();
        var blobProvider = provider.GetRequiredService<IBlobStoreProvider>();
        Assert.Equal("ChainedBlobStoreProvider", blobProvider.GetType().Name);
        return blobProvider;
    }

    private static string NewTempRoot(string label)
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-s3direct-chain-composition", label, Guid.NewGuid().ToString("N"));
    }

    private static void TryDeleteDirectory(string? path)
    {
        if (path is null)
        {
            return;
        }
        try
        {
            if (Directory.Exists(path))
            {
                Directory.Delete(path, recursive: true);
            }
        }
        catch
        {
            // Best-effort cleanup; leftover temp dirs are harmless.
        }
    }

    private sealed class NamedFakeHttpClientFactory : IHttpClientFactory
    {
        private readonly Dictionary<string, HttpClient> _clients = new();

        public NamedFakeHttpClientFactory(IReadOnlyDictionary<string, HttpMessageHandler> handlersByName, string originBaseUrl)
        {
            foreach (var (name, handler) in handlersByName)
            {
                var client = new HttpClient(handler);
                if (name == HttpRemoteBlobStoreProvider.HttpClientName)
                {
                    client.BaseAddress = new Uri(originBaseUrl, UriKind.Absolute);
                }
                _clients[name] = client;
            }
        }

        public HttpClient CreateClient(string name)
        {
            return _clients.TryGetValue(name, out var client)
                ? client
                : throw new InvalidOperationException($"no fake HttpClient registered for '{name}'");
        }
    }

    /// <summary>
    /// Fake origin: relay <c>GET/PUT /blobs/{hash}</c> (always 404 on GET
    /// when <c>relayAlways404</c>, mirroring an S3-backed origin that never
    /// hydrates bytes; always 201 on PUT so the relay link's own push
    /// "succeeds" for the fan-out test) plus the S4.1/4.2 URL-resolution
    /// endpoints (<c>GET .../url</c>, <c>POST .../uploaded</c>), all wired
    /// to a single presigned bucket URL that <see cref="BucketHandler"/>
    /// serves from <see cref="BucketObjectsByHash"/>.
    /// </summary>
    private sealed class FakeOriginHandler : HttpMessageHandler
    {
        private readonly bool _relayAlways404;

        public int RelayGetCallCount { get; private set; }
        public int RelayPutCallCount { get; private set; }
        public int UrlResolveCallCount { get; private set; }
        public int UploadedConfirmCallCount { get; private set; }

        /// <summary>Seeded bucket contents, keyed by hash — what a presigned GET should return.</summary>
        public Dictionary<string, (byte[] Bytes, string? ContentEncoding)> BucketObjectsByHash { get; }

        public FakeOriginHandler(bool relayAlways404, Dictionary<string, (byte[] Bytes, string? ContentEncoding)> bucketBytesByHash)
        {
            _relayAlways404 = relayAlways404;
            BucketObjectsByHash = bucketBytesByHash;
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            var path = request.RequestUri!.AbsolutePath;

            if (path.EndsWith("/url", StringComparison.Ordinal))
            {
                UrlResolveCallCount++;
                var json = JsonSerializer.Serialize(new { url = BucketUrl, expiresAtUtc = "2026-07-15T12:34:56Z" });
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
                {
                    Content = new StringContent(json, Encoding.UTF8, "application/json")
                });
            }

            if (path.EndsWith("/uploaded", StringComparison.Ordinal))
            {
                UploadedConfirmCallCount++;
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK));
            }

            if (request.Method == HttpMethod.Get)
            {
                RelayGetCallCount++;
                return Task.FromResult(new HttpResponseMessage(_relayAlways404 ? HttpStatusCode.NotFound : HttpStatusCode.OK)
                {
                    Content = _relayAlways404 ? null : new ByteArrayContent([])
                });
            }

            if (request.Method == HttpMethod.Put)
            {
                RelayPutCallCount++;
                return Task.FromResult(new HttpResponseMessage(HttpStatusCode.Created));
            }

            throw new InvalidOperationException($"unexpected fake origin request: {request.Method} {path}");
        }
    }

    /// <summary>Serves the presigned bucket URL: GET returns whatever's seeded, PUT records the upload.</summary>
    private sealed class BucketHandler : HttpMessageHandler
    {
        private readonly FakeOriginHandler _origin;

        public int PutCallCount { get; private set; }

        public BucketHandler(FakeOriginHandler origin)
        {
            _origin = origin;
        }

        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            if (request.Method == HttpMethod.Put)
            {
                PutCallCount++;
                // Record whatever was uploaded under every hash the origin
                // was seeded to serve back for a GET — good enough for the
                // fan-out test, which doesn't read the object back.
                var body = request.Content is null ? [] : await request.Content.ReadAsByteArrayAsync(cancellationToken);
                var encoding = request.Content?.Headers.ContentEncoding.FirstOrDefault();
                foreach (var key in _origin.BucketObjectsByHash.Keys.ToArray())
                {
                    _origin.BucketObjectsByHash[key] = (body, encoding);
                }
                return new HttpResponseMessage(HttpStatusCode.OK);
            }

            // GET: return the seeded bucket object for whichever hash this
            // presigned URL was minted for. The fakes here only ever mint
            // one presigned URL per test, so a single-entry lookup by
            // "whatever's seeded" is unambiguous.
            var (bytes, contentEncoding) = _origin.BucketObjectsByHash.Values.First();
            var response = new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new ByteArrayContent(bytes)
            };
            if (contentEncoding is not null)
            {
                response.Content.Headers.ContentEncoding.Add(contentEncoding);
            }
            return response;
        }
    }
}
