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

    private sealed class FakeLink : IBlobStoreProvider
    {
        public BlobReadResult GetResult { get; set; } = BlobReadResult.Missing;
        public Exception? GetException { get; set; }
        public Exception? PutException { get; set; }

        public int GetCallCount { get; private set; }
        public int PutCallCount { get; private set; }

        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            GetCallCount++;
            if (GetException is not null)
            {
                throw GetException;
            }
            return ValueTask.FromResult(GetResult);
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
