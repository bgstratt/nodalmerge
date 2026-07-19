using Microsoft.Extensions.Configuration;

namespace NodalMerge.DotNetHost;

/// <summary>
/// Configuration for the blob origin HTTP surface
/// (<c>GET/HEAD/PUT /blobs/{hash}</c> + <c>/api/blobs/{hash}</c> mirrors) — see
/// docs/BLOB_HTTP_SURFACE.md. When <see cref="AuthToken"/> is unset the surface is
/// anonymous; when set, every request must carry a matching
/// <c>Authorization: Bearer</c> header.
/// </summary>
public sealed record BlobHttpOptions(string? AuthToken, long MaxBlobBytes)
{
    public const string SectionName = "NodalMerge:BlobHttp";

    /// <summary>Default PUT body size cap: 64 MiB (docs/BLOB_HTTP_SURFACE.md §PUT).</summary>
    public const long DefaultMaxBlobBytes = 64L * 1024 * 1024;

    public static BlobHttpOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        var authToken = section?["AuthToken"];
        var maxBlobBytes = DefaultMaxBlobBytes;
        var maxBlobBytesRaw = section?["MaxBlobBytes"];
        if (!string.IsNullOrWhiteSpace(maxBlobBytesRaw) && long.TryParse(maxBlobBytesRaw, out var parsed))
        {
            maxBlobBytes = parsed;
        }

        return new BlobHttpOptions(
            string.IsNullOrWhiteSpace(authToken) ? null : authToken,
            maxBlobBytes
        );
    }

    /// <summary>
    /// Fail fast at startup rather than at request time (slice 4.3,
    /// blob-cas-remediation.md). Before this guard, a misconfigured negative
    /// <see cref="MaxBlobBytes"/> threw an unhandled
    /// <see cref="ArgumentOutOfRangeException"/> from the first chunked PUT's
    /// <c>MemoryStream</c> allocation (a 500, not a config error), and a
    /// misconfigured zero silently 413'd every PUT forever with no indication
    /// why. Matches the exception style every sibling options record in
    /// <c>NodalMerge.Host.Composition</c> already uses
    /// (<c>throw new InvalidOperationException($"{SectionName}:Field ...")</c>
    /// — see e.g. <c>S3DelegatedBlobOptions.Validate()</c>).
    /// </summary>
    public void Validate()
    {
        if (MaxBlobBytes <= 0)
        {
            throw new InvalidOperationException(
                $"{SectionName}:MaxBlobBytes must be a positive number of bytes (got {MaxBlobBytes})"
            );
        }
    }
}
