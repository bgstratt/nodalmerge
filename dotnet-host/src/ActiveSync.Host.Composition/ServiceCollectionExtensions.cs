using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.IdentityModel.Tokens;
using System.IdentityModel.Tokens.Jwt;
using System.Net.Http.Json;
using System.Security.Claims;
using System.Text;

namespace ActiveSync.Host.Composition;

public static class ServiceCollectionExtensions
{
    private static readonly HashSet<string> SupportedNodeProviders =
        ["InMemory", "Sqlite", "Mongo", "Postgres"];

    private static readonly HashSet<string> SupportedBlobProviders =
        ["WsOnly", "File", "S3Direct", "S3Delegated"];

    private static readonly HashSet<string> SupportedAuthProviders =
        ["Default", "JwtBridgeEmbedded", "JwtBridgeSidecar"];

    public static IServiceCollection AddActiveSyncHostProviders(
        this IServiceCollection services,
        IConfiguration? configuration = null
    )
    {
        var options = ActiveSyncHostProviderOptions.FromConfiguration(configuration);
        Validate(options);

        services.AddSingleton(options);

        RegisterNodeProvider(services, options, configuration);
        RegisterBlobProvider(services, options, configuration);
        RegisterAuthProvider(services, options, configuration);
        services.AddSingleton<IProviderHealthCheck, DefaultProviderHealthCheck>();

        return services;
    }

    private static void RegisterAuthProvider(
        IServiceCollection services,
        ActiveSyncHostProviderOptions options,
        IConfiguration? configuration
    )
    {
        if (string.Equals(options.AuthProvider, "Default", StringComparison.Ordinal))
        {
            services.AddSingleton<IRoomTokenAuthProvider, DefaultRoomTokenAuthProvider>();
            return;
        }

        if (string.Equals(options.AuthProvider, "JwtBridgeEmbedded", StringComparison.Ordinal))
        {
            var jwtOptions = JwtBridgeEmbeddedAuthOptions.FromConfiguration(configuration);
            jwtOptions.Validate();

            services.AddSingleton(jwtOptions);
            services.AddSingleton<IRoomTokenAuthProvider, JwtBridgeEmbeddedRoomTokenAuthProvider>();
            return;
        }

        if (string.Equals(options.AuthProvider, "JwtBridgeSidecar", StringComparison.Ordinal))
        {
            var sidecarOptions = JwtBridgeSidecarAuthOptions.FromConfiguration(configuration);
            sidecarOptions.Validate();

            services.AddSingleton(sidecarOptions);
            services.AddHttpClient("ActiveSync.JwtBridgeSidecar", (sp, httpClient) =>
            {
                var opts = sp.GetRequiredService<JwtBridgeSidecarAuthOptions>();
                httpClient.BaseAddress = new Uri(opts.BaseUrl, UriKind.Absolute);
                httpClient.Timeout = TimeSpan.FromSeconds(opts.TimeoutSeconds);

                if (!string.IsNullOrWhiteSpace(opts.ApiKey))
                {
                    httpClient.DefaultRequestHeaders.Remove(opts.ApiKeyHeader);
                    httpClient.DefaultRequestHeaders.Add(opts.ApiKeyHeader, opts.ApiKey);
                }
            });
            services.AddSingleton<IRoomTokenAuthProvider, JwtBridgeSidecarRoomTokenAuthProvider>();
            return;
        }

        throw new InvalidOperationException(
            $"Unsupported auth provider '{options.AuthProvider}'. Supported: {string.Join(", ", SupportedAuthProviders)}"
        );
    }

    private static void Validate(ActiveSyncHostProviderOptions options)
    {
        if (!SupportedNodeProviders.Contains(options.NodeStorageProvider))
        {
            throw new InvalidOperationException(
                $"Unsupported node storage provider '{options.NodeStorageProvider}'. Supported: {string.Join(", ", SupportedNodeProviders)}"
            );
        }

        if (!SupportedBlobProviders.Contains(options.BlobStorageProvider))
        {
            throw new InvalidOperationException(
                $"Unsupported blob storage provider '{options.BlobStorageProvider}'. Supported: {string.Join(", ", SupportedBlobProviders)}"
            );
        }

        if (!SupportedAuthProviders.Contains(options.AuthProvider))
        {
            throw new InvalidOperationException(
                $"Unsupported auth provider '{options.AuthProvider}'. Supported: {string.Join(", ", SupportedAuthProviders)}"
            );
        }
    }

    private static void RegisterNodeProvider(
        IServiceCollection services,
        ActiveSyncHostProviderOptions options,
        IConfiguration? configuration
    )
    {
        if (string.Equals(options.NodeStorageProvider, "InMemory", StringComparison.Ordinal))
        {
            services.AddSingleton<INodeStoreProvider, InMemoryNodeStoreProvider>();
            return;
        }

        if (string.Equals(options.NodeStorageProvider, "Sqlite", StringComparison.Ordinal))
        {
            var sqliteOptions = SqliteNodeStorageOptions.FromConfiguration(configuration);
            sqliteOptions.Validate();
            services.AddSingleton(sqliteOptions);
            services.AddSingleton<INodeStoreProvider, SqliteNodeStoreProvider>();
            return;
        }

        if (string.Equals(options.NodeStorageProvider, "Mongo", StringComparison.Ordinal))
        {
            var mongoOptions = MongoNodeStorageOptions.FromConfiguration(configuration);
            mongoOptions.Validate();
            services.AddSingleton(mongoOptions);
            services.AddSingleton<INodeStoreProvider, MongoNodeStoreProvider>();
            return;
        }

        throw new InvalidOperationException(
            $"Unsupported node storage provider '{options.NodeStorageProvider}'. Supported: {string.Join(", ", SupportedNodeProviders)}"
        );
    }

    private static void RegisterBlobProvider(
        IServiceCollection services,
        ActiveSyncHostProviderOptions options,
        IConfiguration? configuration
    )
    {
        if (string.Equals(options.BlobStorageProvider, "WsOnly", StringComparison.Ordinal))
        {
            services.AddSingleton<WsOnlyBlobStoreProvider>();
            services.AddSingleton<IBlobStoreProvider>(sp => sp.GetRequiredService<WsOnlyBlobStoreProvider>());
            services.AddSingleton<IBlobUrlResolverProvider>(sp => sp.GetRequiredService<WsOnlyBlobStoreProvider>());
            return;
        }

        if (string.Equals(options.BlobStorageProvider, "File", StringComparison.Ordinal))
        {
            var fileOptions = FileBlobStorageOptions.FromConfiguration(configuration);
            fileOptions.Validate();
            services.AddSingleton(fileOptions);
            services.AddSingleton<FileBlobStoreProvider>();
            services.AddSingleton<IBlobStoreProvider>(sp => sp.GetRequiredService<FileBlobStoreProvider>());
            services.AddSingleton<IBlobUrlResolverProvider>(sp => sp.GetRequiredService<FileBlobStoreProvider>());
            return;
        }

        if (string.Equals(options.BlobStorageProvider, "S3Delegated", StringComparison.Ordinal))
        {
            var delegatedOptions = S3DelegatedBlobOptions.FromConfiguration(configuration);
            delegatedOptions.Validate();

            services.AddSingleton(delegatedOptions);
            services.AddHttpClient("ActiveSync.S3DelegatedBlobResolver", (sp, httpClient) =>
            {
                var opts = sp.GetRequiredService<S3DelegatedBlobOptions>();
                httpClient.BaseAddress = new Uri(opts.BaseUrl, UriKind.Absolute);
                httpClient.Timeout = TimeSpan.FromSeconds(opts.TimeoutSeconds);

                if (!string.IsNullOrWhiteSpace(opts.ApiKey))
                {
                    httpClient.DefaultRequestHeaders.Remove(opts.ApiKeyHeader);
                    httpClient.DefaultRequestHeaders.Add(opts.ApiKeyHeader, opts.ApiKey);
                }
            });

            services.AddSingleton<WsOnlyBlobStoreProvider>();
            services.AddSingleton<IBlobStoreProvider>(sp => sp.GetRequiredService<WsOnlyBlobStoreProvider>());
            services.AddSingleton<IBlobUrlResolverProvider, S3DelegatedBlobUrlResolverProvider>();
            return;
        }

        throw new InvalidOperationException(
            $"Unsupported blob storage provider '{options.BlobStorageProvider}'. Supported: {string.Join(", ", SupportedBlobProviders)}"
        );
    }
}

internal sealed class InMemoryNodeStoreProvider : INodeStoreProvider
{
    public ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
    {
        return ValueTask.FromResult<NodeSnapshot?>(null);
    }

    public ValueTask<CompactionSnapshot?> LoadCompactionSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken = default)
    {
        return ValueTask.FromResult<CompactionSnapshot?>(null);
    }

    public ValueTask PersistAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<AcceptedNodeRecord> nodes,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.CompletedTask;
    }

    public ValueTask DeleteAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<string> nodeIdHexes,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.CompletedTask;
    }

    public ValueTask PersistCompactionSnapshotAsync(
        string roomId,
        CompactionSnapshot snapshot,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.CompletedTask;
    }
}

internal sealed class WsOnlyBlobStoreProvider : IBlobStoreProvider, IBlobUrlResolverProvider
{
    private readonly System.Collections.Concurrent.ConcurrentDictionary<string, (byte[] Bytes, string? ContentType)> _blobs =
        new(StringComparer.Ordinal);

    public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        return ValueTask.FromResult(
            _blobs.TryGetValue(hashHex, out var entry)
                ? BlobReadResult.Hit(entry.Bytes, entry.ContentType)
                : BlobReadResult.Missing
        );
    }

    public ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        _blobs[hashHex] = (bytes, contentType);
        return ValueTask.CompletedTask;
    }

    public ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
        BlobPutUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult<PresignedBlobUrl?>(null);
    }

    public ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
        BlobGetUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult<PresignedBlobUrl?>(null);
    }
}

internal sealed class DefaultRoomTokenAuthProvider : IRoomTokenAuthProvider
{
    public ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(RoomTokenValidationResult.Valid);
    }

    public ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(RoomTokenMintResult.NotSupported);
    }
}

internal sealed class JwtBridgeEmbeddedRoomTokenAuthProvider : IRoomTokenAuthProvider
{
    private readonly JwtBridgeEmbeddedAuthOptions _options;
    private readonly SymmetricSecurityKey _signingKey;
    private readonly TokenValidationParameters _validationParameters;
    private readonly JwtSecurityTokenHandler _tokenHandler = new();

    public JwtBridgeEmbeddedRoomTokenAuthProvider(JwtBridgeEmbeddedAuthOptions options)
    {
        _options = options;
        _signingKey = new SymmetricSecurityKey(Encoding.UTF8.GetBytes(_options.SigningKey));
        var previousSigningKeys = _options.PreviousSigningKeys
            .Select(previous => new SymmetricSecurityKey(Encoding.UTF8.GetBytes(previous)))
            .ToArray();

        var allSigningKeys = new SecurityKey[] { _signingKey }
            .Concat(previousSigningKeys)
            .ToArray();

        _validationParameters = new TokenValidationParameters
        {
            ValidateIssuer = true,
            ValidIssuer = _options.Issuer,
            ValidateAudience = true,
            ValidAudience = _options.Audience,
            ValidateIssuerSigningKey = true,
            IssuerSigningKeys = allSigningKeys,
            ValidAlgorithms = new[] { SecurityAlgorithms.HmacSha256 },
            ValidateLifetime = true,
            ClockSkew = TimeSpan.FromSeconds(_options.ClockSkewSeconds)
        };
    }

    public ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        if (string.IsNullOrWhiteSpace(request.SignatureHex))
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("missing embedded token"));
        }

        ClaimsPrincipal principal;
        try
        {
            principal = _tokenHandler.ValidateToken(
                request.SignatureHex,
                _validationParameters,
                out _
            );
        }
        catch
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("invalid embedded token"));
        }

        var room = principal.FindFirst("room")?.Value;
        if (!string.Equals(room, request.RoomId, StringComparison.Ordinal))
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("room mismatch"));
        }

        var peer = principal.FindFirst("peer_pubkey")?.Value;
        if (!string.Equals(peer, request.PeerPubkeyHex, StringComparison.Ordinal))
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("peer mismatch"));
        }

        var expClaim = principal.FindFirst(JwtRegisteredClaimNames.Exp)?.Value;
        if (!long.TryParse(expClaim, out var exp) || exp != request.ExpiryUnixSeconds)
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("expiry mismatch"));
        }

        var tokenCaps = principal.FindAll("cap").Select(x => x.Value).ToHashSet(StringComparer.Ordinal);
        var requestedCaps = request.Capabilities.ToHashSet(StringComparer.Ordinal);
        if (!tokenCaps.SetEquals(requestedCaps))
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("capability mismatch"));
        }

        return ValueTask.FromResult(RoomTokenValidationResult.Valid);
    }

    public ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        var lifetime = request.LifetimeSeconds.GetValueOrDefault(300);
        if (lifetime < 30)
        {
            lifetime = 30;
        }
        if (lifetime > 3600)
        {
            lifetime = 3600;
        }

        var expiry = DateTimeOffset.UtcNow.ToUnixTimeSeconds() + lifetime;
        var caps = (request.RequestedCapabilities ?? ["read:**"])
            .Where(x => !string.IsNullOrWhiteSpace(x))
            .Distinct(StringComparer.Ordinal)
            .ToArray();

        if (caps.Length == 0)
        {
            caps = ["read:**"];
        }

        var expiresAtUtc = DateTimeOffset.FromUnixTimeSeconds(expiry).UtcDateTime;
        var claims = new List<Claim>
        {
            new("room", request.RoomId),
            new("peer_pubkey", request.PeerPubkeyHex)
        };
        claims.AddRange(caps.Select(cap => new Claim("cap", cap)));

        var descriptor = new SecurityTokenDescriptor
        {
            Subject = new ClaimsIdentity(claims),
            Issuer = _options.Issuer,
            Audience = _options.Audience,
            Expires = expiresAtUtc,
            SigningCredentials = new SigningCredentials(_signingKey, SecurityAlgorithms.HmacSha256)
        };
        var token = _tokenHandler.CreateToken(descriptor);
        var jwt = _tokenHandler.WriteToken(token);

        return ValueTask.FromResult(
            new RoomTokenMintResult(
                IsSupported: true,
                PeerPubkeyHex: request.PeerPubkeyHex,
                ExpiryUnixSeconds: expiry,
                Capabilities: caps,
                SignatureHex: jwt
            )
        );
    }
}

internal sealed class JwtBridgeSidecarRoomTokenAuthProvider : IRoomTokenAuthProvider
{
    private const string SidecarHttpClientName = "ActiveSync.JwtBridgeSidecar";

    private readonly IHttpClientFactory _httpClientFactory;

    public JwtBridgeSidecarRoomTokenAuthProvider(IHttpClientFactory httpClientFactory)
    {
        _httpClientFactory = httpClientFactory;
    }

    public async ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        var client = _httpClientFactory.CreateClient(SidecarHttpClientName);
        var payload = new
        {
            room = request.RoomId,
            peer_pubkey_hex = request.PeerPubkeyHex,
            expiry_secs = request.ExpiryUnixSeconds,
            capabilities = request.Capabilities,
            sig_hex = request.SignatureHex
        };

        HttpResponseMessage response;
        try
        {
            response = await client.PostAsJsonAsync("/validate", payload, cancellationToken);
        }
        catch (TaskCanceledException ex) when (!cancellationToken.IsCancellationRequested)
        {
            throw new SidecarAuthProviderException("sidecar validate timeout", isTimeout: true, innerException: ex);
        }
        catch (HttpRequestException ex)
        {
            throw new SidecarAuthProviderException("sidecar validate request failed", isTimeout: false, innerException: ex);
        }

        if (!response.IsSuccessStatusCode)
        {
            throw new SidecarAuthProviderException(
                "sidecar validate returned non-success status",
                isTimeout: false,
                statusCode: (int)response.StatusCode
            );
        }

        var body = await response.Content.ReadFromJsonAsync<SidecarValidateResponse>(cancellationToken: cancellationToken);
        if (body is null)
        {
            throw new SidecarAuthProviderException("sidecar validate returned empty payload", isTimeout: false);
        }

        return new RoomTokenValidationResult(body.Valid, body.Reason);
    }

    public async ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        var client = _httpClientFactory.CreateClient(SidecarHttpClientName);
        var payload = new
        {
            room = request.RoomId,
            peerPubkeyHex = request.PeerPubkeyHex,
            lifetimeSeconds = request.LifetimeSeconds,
            capabilities = request.RequestedCapabilities
        };

        HttpResponseMessage response;
        try
        {
            response = await client.PostAsJsonAsync("/mint", payload, cancellationToken);
        }
        catch (TaskCanceledException ex) when (!cancellationToken.IsCancellationRequested)
        {
            throw new SidecarAuthProviderException("sidecar mint timeout", isTimeout: true, innerException: ex);
        }
        catch (HttpRequestException ex)
        {
            throw new SidecarAuthProviderException("sidecar mint request failed", isTimeout: false, innerException: ex);
        }

        if ((int)response.StatusCode == 501)
        {
            return RoomTokenMintResult.NotSupported;
        }

        if (!response.IsSuccessStatusCode)
        {
            throw new SidecarAuthProviderException(
                "sidecar mint returned non-success status",
                isTimeout: false,
                statusCode: (int)response.StatusCode
            );
        }

        var body = await response.Content.ReadFromJsonAsync<SidecarMintResponse>(cancellationToken: cancellationToken);
        if (body is null)
        {
            throw new SidecarAuthProviderException("sidecar mint returned empty payload", isTimeout: false);
        }

        return new RoomTokenMintResult(
            IsSupported: true,
            PeerPubkeyHex: body.PeerPubkeyHex,
            ExpiryUnixSeconds: body.ExpiryUnixSeconds,
            Capabilities: body.Capabilities,
            SignatureHex: body.SignatureHex
        );
    }
}

internal sealed class SidecarMintResponse
{
    public string PeerPubkeyHex { get; set; } = string.Empty;

    public long ExpiryUnixSeconds { get; set; }

    public string[] Capabilities { get; set; } = [];

    public string SignatureHex { get; set; } = string.Empty;
}

internal sealed class SidecarValidateResponse
{
    public bool Valid { get; set; }

    public string? Reason { get; set; }
}

internal sealed class DefaultProviderHealthCheck : IProviderHealthCheck
{
    public ValueTask<ProviderHealthStatus> CheckAsync(CancellationToken cancellationToken = default)
    {
        return ValueTask.FromResult(new ProviderHealthStatus("default", IsHealthy: true));
    }
}
