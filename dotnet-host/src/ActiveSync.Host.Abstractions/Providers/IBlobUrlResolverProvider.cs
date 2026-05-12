namespace ActiveSync.Host.Abstractions.Providers;

public interface IBlobUrlResolverProvider
{
    ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
        BlobPutUrlRequest request,
        CancellationToken cancellationToken = default
    );

    ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
        BlobGetUrlRequest request,
        CancellationToken cancellationToken = default
    );
}

public sealed record BlobPutUrlRequest(
    string RoomId,
    string Namespace,
    string HashHex,
    long SizeBytes,
    string? ContentType
);

public sealed record BlobGetUrlRequest(string RoomId, string Namespace, string HashHex);

public sealed record PresignedBlobUrl(string Url, DateTimeOffset ExpiresAtUtc);
