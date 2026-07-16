namespace NodalMerge.Host.Abstractions.Providers;

public interface IBlobStoreProvider
{
    ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default);

    ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    );

    /// <summary>
    /// Cheap existence probe (slice 2.1,
    /// nodalmerge-studio/plans/blob-cas-remediation.md) — answers "does this
    /// blob exist?" without necessarily reading, decompressing, verifying, or
    /// caching its bytes. Backs <c>HEAD /blobs/{hash}</c> and PUT's
    /// content-addressed idempotency check, which only ever need a yes/no
    /// answer, not the payload.
    ///
    /// The default implementation falls back to <see cref="TryGetBlobAsync"/>
    /// so this addition is purely additive: a third-party
    /// <see cref="IBlobStoreProvider"/> written before this member existed
    /// still compiles and still answers correctly — just via the same full
    /// read it always did, since it has no cheaper mechanism to fall back to.
    /// Providers that CAN answer more cheaply (local file existence, a bucket
    /// or origin <c>HEAD</c>) should override this to actually get that
    /// benefit — see <c>FileBlobStoreProvider</c>, <c>HttpRemoteBlobStoreProvider</c>,
    /// <c>S3DirectBlobStoreProvider</c>, and <c>ChainedBlobStoreProvider</c>.
    /// </summary>
    async ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        var result = await TryGetBlobAsync(hashHex, cancellationToken);
        return result.Found;
    }
}

public sealed record BlobReadResult(bool Found, byte[]? Bytes, string? ContentType)
{
    public static BlobReadResult Missing { get; } = new(false, null, null);

    public static BlobReadResult Hit(byte[] bytes, string? contentType) => new(true, bytes, contentType);
}
