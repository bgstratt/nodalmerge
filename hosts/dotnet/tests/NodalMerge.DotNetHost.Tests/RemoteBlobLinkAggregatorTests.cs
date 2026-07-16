using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Unit coverage for <see cref="RemoteBlobLinkAggregator"/> — the fallback
/// (GET) + fan-out (PUT) shim slice 4.3 uses to grow the chain to three
/// links (local -&gt; server-relay -&gt; s3-direct) while
/// <see cref="ChainedBlobStoreProvider"/> keeps exactly one BLAKE3 verify
/// gate. Uses simple fake <see cref="IBlobStoreProvider"/> stubs — no HTTP,
/// no DI — since the HTTP-specific behavior of each real link is already
/// covered by <c>HttpRemoteBlobStoreProviderTests</c> and
/// <c>S3DirectBlobStoreProviderTests</c>.
/// </summary>
public sealed class RemoteBlobLinkAggregatorTests
{
    private static readonly string Hash = new('a', 64);

    [Fact]
    public async Task Get_returns_first_links_hit_without_trying_the_second()
    {
        var first = new FakeLink { GetResult = BlobReadResult.Hit([1, 2, 3], null) };
        var second = new FakeLink { GetResult = BlobReadResult.Hit([9, 9, 9], null) };
        var aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        var result = await aggregator.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(new byte[] { 1, 2, 3 }, result.Bytes);
        Assert.Equal(1, first.GetCallCount);
        Assert.Equal(0, second.GetCallCount);
    }

    [Fact]
    public async Task Get_falls_through_to_second_link_when_first_misses()
    {
        var first = new FakeLink { GetResult = BlobReadResult.Missing };
        var second = new FakeLink { GetResult = BlobReadResult.Hit([4, 5, 6], null) };
        var aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        var result = await aggregator.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(new byte[] { 4, 5, 6 }, result.Bytes);
        Assert.Equal(1, first.GetCallCount);
        Assert.Equal(1, second.GetCallCount);
    }

    [Fact]
    public async Task Get_falls_through_to_second_link_when_first_throws()
    {
        var first = new FakeLink { GetException = new InvalidOperationException("degraded") };
        var second = new FakeLink { GetResult = BlobReadResult.Hit([7], null) };
        var aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        var result = await aggregator.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(new byte[] { 7 }, result.Bytes);
    }

    [Fact]
    public async Task Get_returns_missing_when_every_link_misses_or_throws()
    {
        var first = new FakeLink { GetResult = BlobReadResult.Missing };
        var second = new FakeLink { GetException = new InvalidOperationException("also degraded") };
        var aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        var result = await aggregator.TryGetBlobAsync(Hash);

        Assert.False(result.Found);
    }

    [Fact]
    public async Task Put_fans_out_to_every_link()
    {
        var first = new FakeLink();
        var second = new FakeLink();
        var aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        await aggregator.PutBlobAsync(Hash, [1, 2, 3], "text/plain");

        Assert.Equal(1, first.PutCallCount);
        Assert.Equal(1, second.PutCallCount);
    }

    [Fact]
    public async Task Put_one_link_failing_does_not_prevent_the_others_or_throw()
    {
        var first = new FakeLink { PutException = new InvalidOperationException("push failed") };
        var second = new FakeLink();
        var aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        await aggregator.PutBlobAsync(Hash, [1, 2, 3], null);

        Assert.Equal(1, first.PutCallCount);
        Assert.Equal(1, second.PutCallCount);
    }

    /// <summary>
    /// Slice 2.1's benefit is lost for the three-link config unless this class overrides
    /// <c>ExistsAsync</c> itself: <see cref="RemoteBlobLinkAggregator"/> IS the "remote"
    /// argument <see cref="ChainedBlobStoreProvider"/> sees whenever more than one remote
    /// link is configured, so falling through to the interface's compat default would put
    /// a full remote download + write-back back on the HEAD/PUT-idempotency path for
    /// exactly the <c>local -&gt; server-relay -&gt; s3-direct</c> deployment that most
    /// needs the cheap probe. Asserting on <c>GetCallCount == 0</c> (not on the boolean
    /// alone) is what pins that — the default answers the boolean correctly too.
    /// </summary>
    [Fact]
    public async Task Exists_answers_via_cheap_probe_without_hydrating_any_link()
    {
        var first = new FakeLink { Exists = true, GetResult = BlobReadResult.Hit([1, 2, 3], null) };
        var second = new FakeLink { Exists = true, GetResult = BlobReadResult.Hit([9], null) };
        IBlobStoreProvider aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        Assert.True(await aggregator.ExistsAsync(Hash));

        Assert.Equal(0, first.GetCallCount);
        Assert.Equal(0, second.GetCallCount);
        // First link already answered "present" — no reason to probe the second.
        Assert.Equal(1, first.ExistsCallCount);
        Assert.Equal(0, second.ExistsCallCount);
    }

    [Fact]
    public async Task Exists_falls_through_to_second_link_when_first_does_not_have_it()
    {
        var first = new FakeLink { Exists = false };
        var second = new FakeLink { Exists = true };
        IBlobStoreProvider aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        Assert.True(await aggregator.ExistsAsync(Hash));

        Assert.Equal(1, first.ExistsCallCount);
        Assert.Equal(1, second.ExistsCallCount);
        Assert.Equal(0, first.GetCallCount);
        Assert.Equal(0, second.GetCallCount);
    }

    /// <summary>
    /// Mirrors <c>Get_falls_through_to_second_link_when_first_throws</c>: a degraded link
    /// is an availability event, not a verdict on the blob. Same posture as
    /// <see cref="RemoteBlobLinkAggregator.TryGetBlobAsync"/>, deliberately.
    /// </summary>
    [Fact]
    public async Task Exists_falls_through_to_second_link_when_first_throws()
    {
        var first = new FakeLink { ExistsException = new InvalidOperationException("degraded") };
        var second = new FakeLink { Exists = true };
        IBlobStoreProvider aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        Assert.True(await aggregator.ExistsAsync(Hash));
        Assert.Equal(1, second.ExistsCallCount);
    }

    [Fact]
    public async Task Exists_returns_false_when_every_link_misses_or_throws()
    {
        var first = new FakeLink { Exists = false };
        var second = new FakeLink { ExistsException = new InvalidOperationException("also degraded") };
        IBlobStoreProvider aggregator = new RemoteBlobLinkAggregator([first, second], NullLogger<RemoteBlobLinkAggregator>.Instance);

        Assert.False(await aggregator.ExistsAsync(Hash));
        Assert.Equal(0, first.GetCallCount);
        Assert.Equal(0, second.GetCallCount);
    }

    private sealed class FakeLink : IBlobStoreProvider
    {
        public BlobReadResult GetResult { get; set; } = BlobReadResult.Missing;
        public Exception? GetException { get; set; }
        public Exception? PutException { get; set; }
        public bool Exists { get; set; }
        public Exception? ExistsException { get; set; }

        public int GetCallCount { get; private set; }
        public int PutCallCount { get; private set; }
        public int ExistsCallCount { get; private set; }

        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            GetCallCount++;
            if (GetException is not null)
            {
                throw GetException;
            }
            return ValueTask.FromResult(GetResult);
        }

        /// <summary>
        /// Models a link that CAN answer existence cheaply — the real
        /// <see cref="HttpRemoteBlobStoreProvider"/>/<see cref="S3DirectBlobStoreProvider"/>
        /// shape after slice 2.1 (a bucket/relay <c>HEAD</c>). Tracked separately from
        /// <see cref="GetCallCount"/> precisely so a probe can be told apart from a
        /// hydrating read: if the aggregator ever falls back to the interface's default
        /// <c>ExistsAsync</c>, that default routes through <see cref="TryGetBlobAsync"/>
        /// and <see cref="GetCallCount"/> goes up — which is the failure these tests catch.
        /// </summary>
        public ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            ExistsCallCount++;
            if (ExistsException is not null)
            {
                throw ExistsException;
            }
            return ValueTask.FromResult(Exists);
        }

        public ValueTask PutBlobAsync(string hashHex, byte[] bytes, string? contentType, CancellationToken cancellationToken = default)
        {
            PutCallCount++;
            if (PutException is not null)
            {
                throw PutException;
            }
            return ValueTask.CompletedTask;
        }
    }
}
