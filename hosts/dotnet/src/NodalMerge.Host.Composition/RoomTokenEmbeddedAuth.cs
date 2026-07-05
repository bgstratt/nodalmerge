using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.Extensions.Configuration;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Embedded ed25519 RoomToken auth (S2). Unlike <c>JwtBridgeEmbedded</c>
/// (HS256 JWT claims, .NET-local only), this mints and validates the same
/// ed25519-signed <c>RoomToken</c> the Rust server verifies at hello — one
/// crypto implementation for all hosts, reached via the
/// <c>nodalmerge_host_ffi</c> native library that already ships in the
/// Native.* NuGet packages. Requires a native-supported RID (win-x64 /
/// linux-x64 today); use JwtBridgeEmbedded or JwtBridgeSidecar elsewhere.
/// </summary>
public sealed record RoomTokenEmbeddedAuthOptions(
    string RoomSigningKeyHex,
    int DefaultLifetimeSeconds
)
{
    public const string SectionName = "NodalMerge:Auth:RoomTokenEmbedded";

    public static RoomTokenEmbeddedAuthOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);

        var lifetime = 300;
        if (int.TryParse(section?["DefaultLifetimeSeconds"], out var parsed) && parsed > 0)
        {
            lifetime = parsed;
        }

        return new RoomTokenEmbeddedAuthOptions(
            section?["RoomSigningKeyHex"] ?? string.Empty,
            lifetime
        );
    }

    public void Validate()
    {
        if (RoomSigningKeyHex.Length != 64 || !RoomSigningKeyHex.All(Uri.IsHexDigit))
        {
            throw new InvalidOperationException(
                $"{SectionName}:RoomSigningKeyHex must be 64 hex chars (32-byte ed25519 signing-key seed)"
            );
        }
    }
}

internal sealed class RoomTokenEmbeddedRoomTokenAuthProvider : IRoomTokenAuthProvider
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower
    };

    private readonly RoomTokenEmbeddedAuthOptions _options;
    private readonly CapabilityProfileExpander _capabilityProfileExpander;

    public RoomTokenEmbeddedRoomTokenAuthProvider(
        RoomTokenEmbeddedAuthOptions options,
        CapabilityProfileExpander capabilityProfileExpander)
    {
        _options = options;
        _capabilityProfileExpander = capabilityProfileExpander;
    }

    public ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        var lifetime = Math.Clamp(request.LifetimeSeconds.GetValueOrDefault(_options.DefaultLifetimeSeconds), 30, 3600);
        var expiry = DateTimeOffset.UtcNow.ToUnixTimeSeconds() + lifetime;

        var caps = (request.RequestedCapabilities ?? ["read:**"])
            .Where(x => !string.IsNullOrWhiteSpace(x))
            .Distinct(StringComparer.Ordinal)
            .ToArray();
        if (caps.Length == 0)
        {
            caps = ["read:**"];
        }

        if (_capabilityProfileExpander.IsEnabled)
        {
            if (!_capabilityProfileExpander.TryExpand(
                    caps, request.CapabilityProfileVersion, out var expanded, out var expandError))
            {
                throw new InvalidOperationException(expandError ?? "capability expansion failed");
            }
            caps = expanded.ToArray();
        }

        var response = RoomTokenNative.Call<MintResponse>(
            RoomTokenNative.MintJson,
            new
            {
                room_id = request.RoomId,
                room_signing_key_hex = _options.RoomSigningKeyHex,
                peer_pubkey_hex = request.PeerPubkeyHex,
                expiry_unix_secs = expiry,
                capabilities = caps
            }
        );

        return ValueTask.FromResult(new RoomTokenMintResult(
            IsSupported: true,
            PeerPubkeyHex: response.PeerPubkey,
            ExpiryUnixSeconds: (long)response.Expiry,
            Capabilities: response.Caps,
            SignatureHex: response.Sig
        ));
    }

    public ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        if (string.IsNullOrWhiteSpace(request.SignatureHex))
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("missing token signature"));
        }

        var caps = request.Capabilities;
        if (_capabilityProfileExpander.IsEnabled
            && !_capabilityProfileExpander.TryExpand(
                request.Capabilities, request.CapabilityProfileVersion, out caps, out var expandError))
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid(expandError ?? "capability expansion failed"));
        }

        ValidateResponse response;
        try
        {
            response = RoomTokenNative.Call<ValidateResponse>(
                RoomTokenNative.ValidateJson,
                new
                {
                    room_id = request.RoomId,
                    room_signing_key_hex = _options.RoomSigningKeyHex,
                    peer_pubkey_hex = request.PeerPubkeyHex,
                    expiry_unix_secs = request.ExpiryUnixSeconds,
                    capabilities = caps,
                    sig_hex = request.SignatureHex
                }
            );
        }
        catch (RoomTokenNativeException e) when (e.Status == 1 /* invalid arg */)
        {
            return ValueTask.FromResult(RoomTokenValidationResult.Invalid("malformed token"));
        }

        return ValueTask.FromResult(response.Valid
            ? RoomTokenValidationResult.Valid
            : RoomTokenValidationResult.Invalid(response.Reason ?? "invalid token"));
    }

    private sealed record MintResponse(
        [property: JsonPropertyName("peer_pubkey")] string PeerPubkey,
        [property: JsonPropertyName("expiry")] ulong Expiry,
        [property: JsonPropertyName("caps")] string[] Caps,
        [property: JsonPropertyName("sig")] string Sig,
        [property: JsonPropertyName("room_pubkey_hex")] string RoomPubkeyHex
    );

    private sealed record ValidateResponse(
        [property: JsonPropertyName("valid")] bool Valid,
        [property: JsonPropertyName("reason")] string? Reason
    );
}

internal sealed class RoomTokenNativeException(uint status)
    : InvalidOperationException($"nodalmerge_host_ffi room-token call failed with status {status}")
{
    public uint Status { get; } = status;
}

/// <summary>
/// Minimal P/Invoke binding for the RoomToken FFI functions. Lives in
/// Composition (not the DotNetHost Ffi client) because auth providers are
/// registered here; carries its own resolver honoring the same
/// <c>NODALMERGE_HOST_FFI_DLL</c> override the DotNetHost resolver uses,
/// falling back to default NuGet <c>runtimes/&lt;rid&gt;/native</c> probing.
/// </summary>
internal static partial class RoomTokenNative
{
    private const string LibraryName = "nodalmerge_host_ffi";
    private static bool _resolverConfigured;
    private static readonly object ResolverLock = new();

    [StructLayout(LayoutKind.Sequential)]
    internal struct BytesView
    {
        public nint Ptr;
        public nuint Len;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct BytesOwned
    {
        public nint Ptr;
        public nuint Len;
    }

    [DllImport(LibraryName, EntryPoint = "nm_room_token_mint_json")]
    private static extern uint AsRoomTokenMintJson(BytesView request, out BytesOwned outJson);

    [DllImport(LibraryName, EntryPoint = "nm_room_token_validate_json")]
    private static extern uint AsRoomTokenValidateJson(BytesView request, out BytesOwned outJson);

    [DllImport(LibraryName, EntryPoint = "nm_bytes_owned_free")]
    private static extern void AsBytesOwnedFree(BytesOwned bytes);

    internal delegate uint JsonCall(BytesView request, out BytesOwned outJson);

    internal static readonly JsonCall MintJson = AsRoomTokenMintJson;
    internal static readonly JsonCall ValidateJson = AsRoomTokenValidateJson;

    internal static T Call<T>(JsonCall f, object request)
    {
        EnsureResolver();

        var body = JsonSerializer.SerializeToUtf8Bytes(request);
        unsafe
        {
            fixed (byte* p = body)
            {
                var view = new BytesView { Ptr = (nint)p, Len = (nuint)body.Length };
                var status = f(view, out var owned);
                try
                {
                    if (status != 0)
                    {
                        throw new RoomTokenNativeException(status);
                    }

                    var span = new ReadOnlySpan<byte>((void*)owned.Ptr, checked((int)owned.Len));
                    return JsonSerializer.Deserialize<T>(span)
                        ?? throw new RoomTokenNativeException(255);
                }
                finally
                {
                    if (owned.Ptr != 0 && owned.Len != 0)
                    {
                        AsBytesOwnedFree(owned);
                    }
                }
            }
        }
    }

    private static void EnsureResolver()
    {
        if (_resolverConfigured)
        {
            return;
        }

        lock (ResolverLock)
        {
            if (_resolverConfigured)
            {
                return;
            }

            NativeLibrary.SetDllImportResolver(typeof(RoomTokenNative).Assembly, (name, assembly, searchPath) =>
            {
                if (!string.Equals(name, LibraryName, StringComparison.Ordinal))
                {
                    return nint.Zero;
                }

                var overridePath = Environment.GetEnvironmentVariable("NODALMERGE_HOST_FFI_DLL");
                if (!string.IsNullOrWhiteSpace(overridePath)
                    && NativeLibrary.TryLoad(overridePath, out var fromOverride))
                {
                    return fromOverride;
                }

                // Fall back to default probing (NuGet runtimes/<rid>/native,
                // app dir, OS search path).
                return NativeLibrary.TryLoad(name, assembly, searchPath, out var handle)
                    ? handle
                    : nint.Zero;
            });

            _resolverConfigured = true;
        }
    }
}
