using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using Blake3;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Drives the .NET blob origin HTTP surface (<c>GET/HEAD/PUT /blobs/{hash}</c> +
/// <c>/api/blobs/{hash}</c> mirrors, docs/BLOB_HTTP_SURFACE.md) against the frozen
/// golden vectors (<c>engine/commands/blob-http-surface-vectors.v1.json</c>, linked
/// into this project as <c>blob-http-surface-vectors.v1.json</c>). The Rust mirror is
/// <c>server/server/tests/blob_http_surface.rs</c>. If you change the surface, the
/// vectors, both harnesses, and the doc must move together.
/// </summary>
public sealed class BlobHttpSurfaceTests : IAsyncLifetime
{
    private sealed record BlobVector(
        string Id,
        string Method,
        bool Seeded,
        string Hash,
        string Auth,
        bool ServerTokenConfigured,
        int ExpectStatus,
        IReadOnlyDictionary<string, string>? ExpectHeaders,
        string? ExpectBody,
        bool? ExpectStored,
        string? BodyKind,
        string? BodyContent
    );

    private static readonly string SeedContent;
    private static readonly string SeedHash;
    private static readonly string AuthTokenForTests;
    private static readonly long MaxBlobBytesForTests;
    private static readonly IReadOnlyList<BlobVector> Vectors;

    static BlobHttpSurfaceTests()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "blob-http-surface-vectors.v1.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var root = doc.RootElement;

        SeedContent = root.GetProperty("seed_blob").GetProperty("seed_content").GetString()!;
        MaxBlobBytesForTests = root.GetProperty("max_blob_bytes_for_tests").GetInt64();
        AuthTokenForTests = root.GetProperty("auth_token_for_tests").GetString()!;
        SeedHash = Hasher.Hash(Encoding.ASCII.GetBytes(SeedContent)).ToString();

        var vectors = new List<BlobVector>();
        foreach (var el in root.GetProperty("vectors").EnumerateArray())
        {
            IReadOnlyDictionary<string, string>? expectHeaders = null;
            if (el.TryGetProperty("expect_headers", out var headersEl))
            {
                var headers = new Dictionary<string, string>();
                foreach (var prop in headersEl.EnumerateObject())
                {
                    headers[prop.Name] = prop.Value.GetString()!;
                }
                expectHeaders = headers;
            }

            vectors.Add(new BlobVector(
                el.GetProperty("id").GetString()!,
                el.GetProperty("method").GetString()!,
                el.GetProperty("seeded").GetBoolean(),
                el.GetProperty("hash").GetString()!,
                el.GetProperty("auth").GetString()!,
                el.GetProperty("server_token_configured").GetBoolean(),
                el.GetProperty("expect_status").GetInt32(),
                expectHeaders,
                el.TryGetProperty("expect_body", out var bodyEl) ? bodyEl.GetString() : null,
                el.TryGetProperty("expect_stored", out var storedEl) ? storedEl.GetBoolean() : null,
                el.TryGetProperty("body_kind", out var bodyKindEl) ? bodyKindEl.GetString() : null,
                el.TryGetProperty("body_content", out var bodyContentEl) ? bodyContentEl.GetString() : null
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

    private string? _anonRoot;
    private string? _authRoot;
    private WebApplication? _anonApp;
    private WebApplication? _authApp;
    private HttpClient? _anonClient;
    private HttpClient? _authClient;

    public async Task InitializeAsync()
    {
        _anonRoot = NewTempRoot();
        _authRoot = NewTempRoot();

        _anonApp = BuildApp(_anonRoot, authToken: null);
        _authApp = BuildApp(_authRoot, authToken: AuthTokenForTests);

        await _anonApp.StartAsync();
        await _authApp.StartAsync();

        _anonClient = _anonApp.GetTestClient();
        _authClient = _authApp.GetTestClient();
    }

    public async Task DisposeAsync()
    {
        _anonClient?.Dispose();
        _authClient?.Dispose();

        if (_anonApp is not null)
        {
            await _anonApp.StopAsync();
            await _anonApp.DisposeAsync();
        }
        if (_authApp is not null)
        {
            await _authApp.StopAsync();
            await _authApp.DisposeAsync();
        }

        TryDeleteDirectory(_anonRoot);
        TryDeleteDirectory(_authRoot);
    }

    [Theory]
    [MemberData(nameof(VectorIds))]
    public async Task Vector_matches_contract(string id)
    {
        var vector = Vectors.Single(v => v.Id == id);
        await RunVectorAsync(vector, "/blobs");
    }

    [Fact]
    public async Task Api_blobs_mirror_serves_get_found_and_put_new()
    {
        await RunVectorAsync(Vectors.Single(v => v.Id == "get-found"), "/api/blobs");
        await RunVectorAsync(Vectors.Single(v => v.Id == "put-new"), "/api/blobs");
    }

    /// <summary>
    /// Slice 3.2 (nodalmerge-studio/plans/blob-cas-remediation.md, finding
    /// #13): the identity-stored on-disk file is tampered AFTER being
    /// seeded, so it no longer matches its own filename hash. Before the
    /// fix, <c>FileBlobStoreProvider.TryGetBlobAsync</c>'s identity branch
    /// read the file and returned it unverified, so the origin served the
    /// tampered bytes as a 200. This asserts the origin-level defect
    /// docs/BLOB_HTTP_SURFACE.md guards against directly: "corrupt → 404,
    /// never wrong bytes" — matching what the zstd path and the Rust origin
    /// already do.
    /// </summary>
    [Fact]
    public async Task Get_tampered_identity_blob_returns_404_not_corrupt_200()
    {
        SeedBlob(_anonRoot!, SeedHash, Encoding.ASCII.GetBytes(SeedContent));

        // Tamper the identity file in place, at the path keyed by the
        // ORIGINAL (seed) hash.
        var identityPath = Path.Combine(_anonRoot!, "blake3", SeedHash);
        await File.WriteAllBytesAsync(identityPath, Encoding.ASCII.GetBytes("substituted-attacker-bytes-of-a-different-length"));

        using var response = await _anonClient!.GetAsync($"/blobs/{SeedHash}");

        Assert.True(
            response.StatusCode == HttpStatusCode.NotFound,
            $"expected 404 for a tampered identity blob, got {(int)response.StatusCode}"
        );

        // Belt-and-suspenders: whatever the 404 body is, it must not be the
        // tampered bytes verbatim (the pre-fix bug served them as a 200).
        var bodyText = await response.Content.ReadAsStringAsync();
        Assert.DoesNotContain("substituted-attacker-bytes", bodyText, StringComparison.Ordinal);
    }

    private async Task RunVectorAsync(BlobVector vector, string basePath)
    {
        var (client, root) = vector.ServerTokenConfigured
            ? (_authClient!, _authRoot!)
            : (_anonClient!, _anonRoot!);

        byte[]? bodyBytes = null;
        string requestHash;

        if (vector.Hash == "seed")
        {
            requestHash = SeedHash;
        }
        else if (vector.Hash == "computed-from-body")
        {
            bodyBytes = ResolveBody(vector);
            requestHash = Hasher.Hash(bodyBytes!).ToString();
        }
        else
        {
            requestHash = vector.Hash;
        }

        if (vector.Seeded)
        {
            SeedBlob(root, SeedHash, Encoding.ASCII.GetBytes(SeedContent));
        }

        if (string.Equals(vector.Method, "PUT", StringComparison.Ordinal))
        {
            bodyBytes ??= ResolveBody(vector);
        }

        using var request = new HttpRequestMessage(new HttpMethod(vector.Method), $"{basePath}/{requestHash}");
        if (bodyBytes is not null)
        {
            request.Content = new ByteArrayContent(bodyBytes);
        }
        ApplyAuth(request, vector.Auth);

        using var response = await client.SendAsync(request);

        Assert.True(
            (int)response.StatusCode == vector.ExpectStatus,
            $"vector `{vector.Id}` ({basePath}): expected status {vector.ExpectStatus}, got {(int)response.StatusCode}"
        );

        if (vector.ExpectHeaders is not null)
        {
            foreach (var (headerName, expectedTemplate) in vector.ExpectHeaders)
            {
                var expected = expectedTemplate.Replace("<seed-hash>", SeedHash);
                var actual = GetRawHeaderValue(response, headerName);
                Assert.True(
                    expected == actual,
                    $"vector `{vector.Id}` ({basePath}): header `{headerName}` expected `{expected}`, got `{actual}`"
                );
            }
        }

        if (vector.ExpectBody == "seed-bytes")
        {
            var actualBytes = await response.Content.ReadAsByteArrayAsync();
            Assert.Equal(Encoding.ASCII.GetBytes(SeedContent), actualBytes);
        }
        else if (vector.ExpectBody == "empty")
        {
            var actualBytes = await response.Content.ReadAsByteArrayAsync();
            Assert.Empty(actualBytes);
        }

        if (vector.ExpectStored.HasValue)
        {
            var stored = File.Exists(Path.Combine(root, "blake3", requestHash));
            Assert.True(
                vector.ExpectStored.Value == stored,
                $"vector `{vector.Id}` ({basePath}): expected_stored={vector.ExpectStored.Value}, actual stored={stored}"
            );
        }
    }

    private static byte[]? ResolveBody(BlobVector vector)
    {
        return vector.BodyKind switch
        {
            null => null,
            "seeded" => Encoding.ASCII.GetBytes(SeedContent),
            "new-content" => Encoding.ASCII.GetBytes(vector.BodyContent!),
            "wrong-bytes" => Encoding.ASCII.GetBytes(vector.BodyContent!),
            "oversize" => new byte[MaxBlobBytesForTests + 1],
            _ => throw new InvalidOperationException($"unknown body_kind '{vector.BodyKind}'")
        };
    }

    private static void ApplyAuth(HttpRequestMessage request, string auth)
    {
        switch (auth)
        {
            case "none":
                break;
            case "bearer-wrong":
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", "wrong-token");
                break;
            case "bearer-correct":
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", AuthTokenForTests);
                break;
            default:
                throw new InvalidOperationException($"unknown auth '{auth}'");
        }
    }

    private static string? GetRawHeaderValue(HttpResponseMessage response, string name)
    {
        if (response.Headers.TryGetValues(name, out var values))
        {
            return values.FirstOrDefault();
        }
        if (response.Content.Headers.TryGetValues(name, out var contentValues))
        {
            return contentValues.FirstOrDefault();
        }
        return null;
    }

    private static void SeedBlob(string root, string hash, byte[] bytes)
    {
        var dir = Path.Combine(root, "blake3");
        Directory.CreateDirectory(dir);
        var path = Path.Combine(dir, hash);
        if (!File.Exists(path))
        {
            File.WriteAllBytes(path, bytes);
        }
    }

    private static WebApplication BuildApp(string root, string? authToken)
    {
        return HostApplication.Build(
            [],
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg =>
            {
                var settings = new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = root,
                    ["NodalMerge:BlobHttp:MaxBlobBytes"] = MaxBlobBytesForTests.ToString()
                };
                if (authToken is not null)
                {
                    settings["NodalMerge:BlobHttp:AuthToken"] = authToken;
                }
                cfg.AddInMemoryCollection(settings);
            }
        );
    }

    private static string NewTempRoot()
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-blob-http-surface", Guid.NewGuid().ToString("N"));
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
}
