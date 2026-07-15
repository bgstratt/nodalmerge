namespace NodalMerge.Host.Abstractions.Providers;

/// <summary>
/// Optional seam for blob stores that can serve their at-rest encoding
/// as-is (docs/BLOB_STORAGE_LAYOUT.md §8 / BLOB_HTTP_SURFACE.md
/// "Content encoding"), so the HTTP GET handler can honor
/// <c>Accept-Encoding: zstd</c> without decompress-then-recompress. A
/// provider that doesn't implement this interface is simply treated as
/// identity-only by callers.
/// </summary>
public interface IEncodedBlobSource
{
    ValueTask<EncodedBlobResult> TryGetEncodedBlobAsync(string hashHex, CancellationToken ct = default);
}

/// <summary>
/// <paramref name="ContentEncoding"/> is <c>"zstd"</c> when
/// <paramref name="Bytes"/> are the raw stored zstd frame (the caller MUST
/// NOT decompress before serving it — that's the point), or <c>null</c>
/// when <paramref name="Bytes"/> are identity bytes.
/// </summary>
public readonly record struct EncodedBlobResult(bool Found, byte[]? Bytes, string? ContentEncoding)
{
    public static EncodedBlobResult Missing { get; } = new(false, null, null);

    public static EncodedBlobResult Identity(byte[] bytes) => new(true, bytes, null);

    public static EncodedBlobResult Zstd(byte[] compressedBytes) => new(true, compressedBytes, "zstd");
}
