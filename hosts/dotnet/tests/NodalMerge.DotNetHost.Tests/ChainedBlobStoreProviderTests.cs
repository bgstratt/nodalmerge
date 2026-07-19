using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Unit-level coverage of <see cref="ChainedBlobStoreProvider"/> against
/// counting/corrupting/throwing fakes for local and remote — no real HTTP or
/// disk involved. The HTTP-backed acceptance path lives in
/// <c>ChainedBlobStoreProviderHttpIntegrationTests</c>.
/// </summary>
public sealed class ChainedBlobStoreProviderTests
{
    private static readonly string HashA = new('a', 64);
    private static readonly string HashB = new('b', 64);
    private static readonly string HashC = new('c', 64);
    private static readonly string HashD = new('d', 64);

    [Fact]
    public async Task Local_hit_makes_zero_remote_calls()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new CountingBlobStoreProvider();
        var bytes = new byte[] { 1, 2, 3 };
        await local.PutBlobAsync(HashA, bytes, null, CancellationToken.None);

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        var result = await chain.TryGetBlobAsync(HashA);

        Assert.True(result.Found);
        Assert.Equal(bytes, result.Bytes);
        Assert.Equal(0, remote.GetCalls);
    }

    [Fact]
    public async Task Miss_fetches_from_remote_verifies_and_writes_through()
    {
        var bytes = System.Text.Encoding.UTF8.GetBytes("hello world");
        var hash = Blake3.Hasher.Hash(bytes).ToString();

        var local = new CountingBlobStoreProvider();
        var remote = new CountingBlobStoreProvider();
        await remote.PutBlobAsync(hash, bytes, "text/plain", CancellationToken.None);

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        var result = await chain.TryGetBlobAsync(hash);

        Assert.True(result.Found);
        Assert.Equal(bytes, result.Bytes);
        Assert.Equal(1, remote.GetCalls);

        // Write-through: the local store now has it too.
        var localResult = await local.TryGetBlobAsync(hash);
        Assert.True(localResult.Found);
        Assert.Equal(bytes, localResult.Bytes);
    }

    [Fact]
    public async Task Corrupted_remote_payload_is_rejected_and_local_stays_empty()
    {
        var hash = Blake3.Hasher.Hash(System.Text.Encoding.UTF8.GetBytes("correct bytes")).ToString();
        var wrongBytes = System.Text.Encoding.UTF8.GetBytes("wrong bytes entirely");

        var local = new CountingBlobStoreProvider();
        var remote = new CorruptingBlobStoreProvider(hash, wrongBytes);

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        var result = await chain.TryGetBlobAsync(hash);

        Assert.False(result.Found);
        var localResult = await local.TryGetBlobAsync(hash);
        Assert.False(localResult.Found);
    }

    [Fact]
    public async Task Put_writes_local_then_remote()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new CountingBlobStoreProvider();
        var bytes = new byte[] { 9, 8, 7 };

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        await chain.PutBlobAsync(HashB, bytes, null, CancellationToken.None);

        var localResult = await local.TryGetBlobAsync(HashB);
        var remoteResult = await remote.TryGetBlobAsync(HashB);
        Assert.True(localResult.Found);
        Assert.True(remoteResult.Found);
        Assert.Equal(1, remote.PutCalls);
    }

    [Fact]
    public async Task Remote_put_failure_is_swallowed_and_local_write_stays_intact()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new ThrowingBlobStoreProvider();
        var bytes = new byte[] { 1 };

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);

        // Must not throw — the local write already succeeded.
        await chain.PutBlobAsync(HashC, bytes, null, CancellationToken.None);

        var localResult = await local.TryGetBlobAsync(HashC);
        Assert.True(localResult.Found);
        Assert.Equal(bytes, localResult.Bytes);
    }

    [Fact]
    public async Task Remote_exception_on_get_returns_missing()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new ThrowingBlobStoreProvider();

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        var result = await chain.TryGetBlobAsync(HashD);

        Assert.False(result.Found);
    }

    /// <summary>
    /// Slice 2.1 (nodalmerge-studio/plans/blob-cas-remediation.md):
    /// <see cref="ChainedBlobStoreProvider.ExistsAsync"/> must answer a local
    /// hit without ever touching the remote link or either side's full-read
    /// method — the whole point of the cheap probe.
    /// </summary>
    [Fact]
    public async Task ExistsAsync_local_hit_makes_zero_remote_calls_and_zero_full_reads()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new CountingBlobStoreProvider();
        await local.PutBlobAsync(HashA, [1, 2, 3], null, CancellationToken.None);

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        var exists = await chain.ExistsAsync(HashA);

        Assert.True(exists);
        Assert.Equal(0, local.GetCalls);
        Assert.Equal(0, remote.ExistsCalls);
        Assert.Equal(0, remote.GetCalls);
    }

    /// <summary>
    /// A local miss must fall through to the remote's own cheap probe — not
    /// to a full remote <c>TryGetBlobAsync</c> read/write-back.
    /// </summary>
    [Fact]
    public async Task ExistsAsync_local_miss_falls_through_to_remote_exists_probe_without_a_full_read()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new CountingBlobStoreProvider();
        await remote.PutBlobAsync(HashB, [4, 5, 6], null, CancellationToken.None);

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);
        var exists = await chain.ExistsAsync(HashB);

        Assert.True(exists);
        Assert.Equal(1, remote.ExistsCalls);
        Assert.Equal(0, remote.GetCalls);
        // Existence-only: must NOT write through to local the way a real
        // TryGetBlobAsync miss-then-fetch would.
        Assert.False((await local.TryGetBlobAsync(HashB)).Found);
    }

    [Fact]
    public async Task ExistsAsync_neither_side_has_it_returns_false()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new CountingBlobStoreProvider();

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);

        Assert.False(await chain.ExistsAsync(HashC));
    }

    [Fact]
    public async Task ExistsAsync_remote_probe_exception_is_swallowed_and_returns_false()
    {
        var local = new CountingBlobStoreProvider();
        var remote = new ThrowingBlobStoreProvider();

        var chain = new ChainedBlobStoreProvider(local, remote, NullLogger<ChainedBlobStoreProvider>.Instance);

        // Must not throw — a degraded remote just means "not confirmed
        // present", matching TryGetBlobAsync's local-first-availability
        // stance for the same failure.
        Assert.False(await chain.ExistsAsync(HashD));
    }

    private sealed class CountingBlobStoreProvider : IBlobStoreProvider
    {
        private readonly Dictionary<string, (byte[] Bytes, string? ContentType)> _blobs = new(StringComparer.Ordinal);

        public int GetCalls { get; private set; }
        public int PutCalls { get; private set; }
        public int ExistsCalls { get; private set; }

        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            GetCalls++;
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
            PutCalls++;
            _blobs[hashHex] = (bytes, contentType);
            return ValueTask.CompletedTask;
        }

        /// <summary>
        /// Explicit cheap override so tests can prove
        /// <see cref="ChainedBlobStoreProvider.ExistsAsync"/> routes through
        /// this rather than falling back to <see cref="TryGetBlobAsync"/> —
        /// see the analogous note on
        /// <c>NonHydratingBlobBackendConformanceTests.CountingBlobStoreProvider</c>.
        /// </summary>
        public ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            ExistsCalls++;
            return ValueTask.FromResult(_blobs.ContainsKey(hashHex));
        }
    }

    private sealed class CorruptingBlobStoreProvider(string hash, byte[] wrongBytes) : IBlobStoreProvider
    {
        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            return ValueTask.FromResult(
                string.Equals(hashHex, hash, StringComparison.Ordinal)
                    ? BlobReadResult.Hit(wrongBytes, null)
                    : BlobReadResult.Missing
            );
        }

        public ValueTask PutBlobAsync(
            string hashHex,
            byte[] bytes,
            string? contentType,
            CancellationToken cancellationToken = default
        ) => ValueTask.CompletedTask;
    }

    private sealed class ThrowingBlobStoreProvider : IBlobStoreProvider
    {
        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
            => throw new InvalidOperationException("remote unavailable");

        public ValueTask PutBlobAsync(
            string hashHex,
            byte[] bytes,
            string? contentType,
            CancellationToken cancellationToken = default
        ) => throw new InvalidOperationException("remote unavailable");
    }
}
