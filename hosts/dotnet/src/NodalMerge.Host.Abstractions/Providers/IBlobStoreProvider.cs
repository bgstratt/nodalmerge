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
}

public sealed record BlobReadResult(bool Found, byte[]? Bytes, string? ContentType)
{
    public static BlobReadResult Missing { get; } = new(false, null, null);

    public static BlobReadResult Hit(byte[] bytes, string? contentType) => new(true, bytes, contentType);
}
