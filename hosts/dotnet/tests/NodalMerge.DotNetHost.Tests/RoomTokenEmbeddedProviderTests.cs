using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Exercises the S2 embedded ed25519 RoomToken provider end-to-end through
/// the real native library (mint via FFI, validate via FFI). Cross-runtime
/// interop with the Rust server's verification path is proven on the Rust
/// side in <c>engine/host-ffi/tests/room_token_ffi.rs</c> (FFI-minted token
/// verified via <c>nodalmerge_core::RoomToken::verify</c>, which is exactly
/// what the server runs at hello).
///
/// Requires the native library: resolved via NODALMERGE_HOST_FFI_DLL or
/// default probing, same as the other FFI tests in this suite.
/// </summary>
public sealed class RoomTokenEmbeddedProviderTests
{
    // Deterministic test seed (all 0x11) — same shape as core's token tests.
    private const string RoomSigningKeyHex =
        "1111111111111111111111111111111111111111111111111111111111111111";

    // Any 32-byte value is a syntactically valid peer id for token purposes.
    private const string PeerPubkeyHex =
        "2222222222222222222222222222222222222222222222222222222222222222";

    private static IRoomTokenAuthProvider CreateProvider()
    {
        // Resolve through the real registration path so provider selection is
        // covered too.
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:Auth"] = "RoomTokenEmbedded",
                ["NodalMerge:Auth:RoomTokenEmbedded:RoomSigningKeyHex"] = RoomSigningKeyHex
            })
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(config);
        var provider = services.BuildServiceProvider().GetRequiredService<IRoomTokenAuthProvider>();
        Assert.Equal("RoomTokenEmbeddedRoomTokenAuthProvider", provider.GetType().Name);
        return provider;
    }

    [Fact]
    public async Task MintThenValidateRoundtrips()
    {
        var provider = CreateProvider();

        var minted = await provider.MintAsync(new RoomTokenMintRequest(
            RoomId: "room-a",
            PeerPubkeyHex: PeerPubkeyHex,
            LifetimeSeconds: 120,
            RequestedCapabilities: ["write:intent/**", "read:world/**"],
            CapabilityProfileVersion: null
        ));

        Assert.True(minted.IsSupported);
        Assert.Equal(PeerPubkeyHex, minted.PeerPubkeyHex);
        Assert.Equal(128, minted.SignatureHex.Length); // ed25519 sig, not a JWT
        // Caps come back sorted (they're part of the signed message).
        Assert.Equal(["read:world/**", "write:intent/**"], minted.Capabilities);

        var validated = await provider.ValidateAsync(new RoomTokenValidationRequest(
            RoomId: "room-a",
            PeerPubkeyHex: minted.PeerPubkeyHex,
            ExpiryUnixSeconds: minted.ExpiryUnixSeconds,
            Capabilities: minted.Capabilities,
            CapabilityProfileVersion: null,
            SignatureHex: minted.SignatureHex
        ));

        Assert.True(validated.IsValid, validated.Reason);
    }

    [Fact]
    public async Task TamperedCapabilitiesAreRejected()
    {
        var provider = CreateProvider();
        var minted = await provider.MintAsync(new RoomTokenMintRequest(
            "room-a", PeerPubkeyHex, 120, ["read:world/**"], null));

        var validated = await provider.ValidateAsync(new RoomTokenValidationRequest(
            "room-a", minted.PeerPubkeyHex, minted.ExpiryUnixSeconds,
            ["write:world/**"], null, minted.SignatureHex));

        Assert.False(validated.IsValid);
    }

    [Fact]
    public async Task WrongRoomIsRejected()
    {
        var provider = CreateProvider();
        var minted = await provider.MintAsync(new RoomTokenMintRequest(
            "room-a", PeerPubkeyHex, 120, ["read:world/**"], null));

        var validated = await provider.ValidateAsync(new RoomTokenValidationRequest(
            "room-b", minted.PeerPubkeyHex, minted.ExpiryUnixSeconds,
            minted.Capabilities, null, minted.SignatureHex));

        Assert.False(validated.IsValid);
    }

    [Fact]
    public async Task ExpiredTokenIsRejected()
    {
        var provider = CreateProvider();
        var minted = await provider.MintAsync(new RoomTokenMintRequest(
            "room-a", PeerPubkeyHex, 120, ["read:world/**"], null));

        var validated = await provider.ValidateAsync(new RoomTokenValidationRequest(
            "room-a", minted.PeerPubkeyHex,
            ExpiryUnixSeconds: 1, // long past
            minted.Capabilities, null, minted.SignatureHex));

        Assert.False(validated.IsValid);
    }

    [Fact]
    public async Task MalformedSignatureIsRejectedNotThrown()
    {
        var provider = CreateProvider();

        var validated = await provider.ValidateAsync(new RoomTokenValidationRequest(
            "room-a", PeerPubkeyHex, long.MaxValue, ["read:world/**"], null, "not-hex"));

        Assert.False(validated.IsValid);
    }

    [Fact]
    public void OptionsRejectBadSigningKey()
    {
        Assert.Throws<InvalidOperationException>(
            () => new RoomTokenEmbeddedAuthOptions("too-short", 300).Validate());
    }
}
