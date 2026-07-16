using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Pins the ⚠ compat guarantee of slice 2.1
/// (nodalmerge-studio/plans/blob-cas-remediation.md): adding
/// <c>IBlobStoreProvider.ExistsAsync</c> is additive — a third-party
/// implementation written before this slice, which implements ONLY the two
/// original members (<c>TryGetBlobAsync</c>/<c>PutBlobAsync</c>), must still
/// compile against the interface and must still answer existence correctly
/// via the interface's default implementation (which falls back to
/// <c>TryGetBlobAsync</c>).
///
/// Before slice 2.1, this file fails to BUILD (not just fails at runtime):
/// <see cref="LegacyTwoMemberBlobStoreProvider"/> has no <c>ExistsAsync</c>
/// member at all, so <c>provider.ExistsAsync(...)</c> below is a compile
/// error (CS1061). That compile failure IS this test's RED proof — see the
/// slice 2.1 report for the literal `dotnet build` output.
/// </summary>
public sealed class BlobStoreProviderExistsAsyncDefaultTests
{
    /// <summary>
    /// Deliberately implements ONLY <see cref="IBlobStoreProvider.TryGetBlobAsync"/>
    /// and <see cref="IBlobStoreProvider.PutBlobAsync"/> — no <c>ExistsAsync</c>
    /// override — standing in for an out-of-tree provider that predates this
    /// slice and never gets updated.
    /// </summary>
    private sealed class LegacyTwoMemberBlobStoreProvider : IBlobStoreProvider
    {
        private readonly Dictionary<string, byte[]> _blobs = new(StringComparer.Ordinal);

        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            return ValueTask.FromResult(
                _blobs.TryGetValue(hashHex, out var bytes)
                    ? BlobReadResult.Hit(bytes, contentType: null)
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
            _blobs[hashHex] = bytes;
            return ValueTask.CompletedTask;
        }
    }

    [Fact]
    public async Task Default_ExistsAsync_reports_true_for_a_stored_blob_on_a_two_member_provider()
    {
        IBlobStoreProvider provider = new LegacyTwoMemberBlobStoreProvider();
        var hash = new string('a', 64);
        await provider.PutBlobAsync(hash, [1, 2, 3], null, CancellationToken.None);

        Assert.True(await provider.ExistsAsync(hash));
    }

    [Fact]
    public async Task Default_ExistsAsync_reports_false_for_a_blob_the_provider_never_stored()
    {
        IBlobStoreProvider provider = new LegacyTwoMemberBlobStoreProvider();

        Assert.False(await provider.ExistsAsync(new string('b', 64)));
    }
}
