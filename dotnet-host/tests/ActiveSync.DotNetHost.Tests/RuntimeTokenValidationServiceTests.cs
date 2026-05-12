using ActiveSync.DotNetHost.Runtime;
using ActiveSync.Host.Abstractions.Providers;

namespace ActiveSync.DotNetHost.Tests;

public sealed class RuntimeTokenValidationServiceTests
{
    [Fact]
    public async Task ValidateInboundAsync_allows_when_no_token_present()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.True(result.Allowed);
        Assert.Null(result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_denies_when_token_missing_sig()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000}}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.False(result.Allowed);
        Assert.Equal("hello.token.sig is required", result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_denies_when_provider_rejects_token()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Invalid("expired"))
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.False(result.Allowed);
        Assert.Equal("token rejected: expired", result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_allows_when_provider_accepts_token()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"client-hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.True(result.Allowed);
        Assert.Null(result.ErrorMessage);
    }
}

internal sealed class RuntimeTokenValidationAuthProviderStub : IRoomTokenAuthProvider
{
    private readonly RoomTokenValidationResult _validationResult;

    public RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult validationResult)
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
