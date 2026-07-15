using System.Net.Http;
using ZstdSharp;

namespace NodalMerge.Host.Composition;

/// <summary>
/// zstd at-rest / on-the-wire encoding helpers shared by every blob provider
/// that opts into compression (docs/BLOB_STORAGE_LAYOUT.md §8):
/// <see cref="FileBlobStoreProvider"/> (at rest) and, since slice 4.3,
/// <see cref="S3DirectBlobStoreProvider"/> (client-side, before a presigned
/// PUT — "S3 won't compress for you"). A blob's identity is always Blake3 of
/// its uncompressed bytes — these helpers only decide whether a write is
/// worth encoding and perform the encode/decode; hash verification stays
/// with the caller.
/// </summary>
internal static class BlobCompression
{
    /// <summary>Sample at most this many bytes when deciding whether a payload is worth compressing.</summary>
    private const int SampleSize = 64 * 1024;

    /// <summary>
    /// Recommended skip heuristic (docs/BLOB_STORAGE_LAYOUT.md §8, guidance
    /// not contract): skip declared compressed-media content types, never
    /// compress below the configured minimum, and skip payloads whose
    /// sampled compression ratio doesn't clear the configured minimum gain.
    /// Takes the two knobs directly (rather than a whole options record) so
    /// every caller — <see cref="FileBlobStoreProvider"/>'s
    /// <see cref="FileBlobStorageOptions"/> and
    /// <see cref="S3DirectBlobStoreProvider"/>'s
    /// <see cref="S3DirectBlobOriginOptions"/> alike — can reuse the exact
    /// same heuristic without either duplicating it or depending on the
    /// other's options type.
    /// </summary>
    internal static bool ShouldCompress(byte[] bytes, string? contentType, int compressionMinBytes, int compressionLevel)
    {
        if (bytes.Length < compressionMinBytes)
        {
            return false;
        }

        if (IsCompressedMediaContentType(contentType))
        {
            return false;
        }

        var sampleLength = Math.Min(SampleSize, bytes.Length);
        byte[] compressedSample;
        using (var compressor = new Compressor(compressionLevel))
        {
            compressedSample = compressor.Wrap(bytes.AsSpan(0, sampleLength)).ToArray();
        }

        var ratio = (double)compressedSample.Length / sampleLength;
        return ratio <= 0.98;
    }

    /// <summary>
    /// image/*, video/*, audio/*, and the common already-compressed
    /// application types called out in the layout doc's skip heuristic.
    /// </summary>
    internal static bool IsCompressedMediaContentType(string? contentType)
    {
        if (string.IsNullOrWhiteSpace(contentType))
        {
            return false;
        }

        // Strip any "; charset=..." style parameter before matching.
        var semicolon = contentType.IndexOf(';');
        var type = (semicolon >= 0 ? contentType[..semicolon] : contentType).Trim().ToLowerInvariant();

        if (type.StartsWith("image/", StringComparison.Ordinal)
            || type.StartsWith("video/", StringComparison.Ordinal)
            || type.StartsWith("audio/", StringComparison.Ordinal))
        {
            return true;
        }

        return type is "application/zip" or "application/gzip" or "application/zstd" or "application/wasm"
            || type.StartsWith("application/x-7z", StringComparison.Ordinal);
    }

    /// <summary>Encodes <paramref name="bytes"/> as a single zstd frame.</summary>
    internal static byte[] Compress(byte[] bytes, int level)
    {
        using var compressor = new Compressor(level);
        return compressor.Wrap(bytes).ToArray();
    }

    /// <summary>
    /// Decodes a single zstd frame. Returns <c>null</c> — never throws — on
    /// any corrupt/truncated/non-zstd input, so callers can treat a bad
    /// frame as a missing blob (docs/BLOB_STORAGE_LAYOUT.md §8: "a corrupt
    /// frame ... is treated as a missing blob, never served").
    /// </summary>
    internal static byte[]? TryDecompress(byte[] compressed)
    {
        try
        {
            var decompressedSize = Decompressor.GetDecompressedSize(compressed);
            if (decompressedSize > int.MaxValue)
            {
                // Not a realistic blob size — treat as corrupt rather than
                // attempting a huge allocation.
                return null;
            }

            var buffer = new byte[(int)decompressedSize];
            using var decompressor = new Decompressor();
            var written = decompressor.Unwrap((ReadOnlySpan<byte>)compressed, (Span<byte>)buffer);
            return written == buffer.Length ? buffer : null;
        }
        catch (ZstdException)
        {
            return null;
        }
        catch (ArgumentOutOfRangeException)
        {
            return null;
        }
        catch (OverflowException)
        {
            return null;
        }
    }

    /// <summary>
    /// True when an HTTP response's <c>Content-Encoding</c> header lists
    /// <c>zstd</c> (docs/BLOB_HTTP_SURFACE.md "Content encoding"). Shared by
    /// <see cref="HttpRemoteBlobStoreProvider"/> (the relay's own
    /// negotiated encoding) and <see cref="S3DirectBlobStoreProvider"/>
    /// (the bucket's stored object-metadata encoding, set at PUT time by
    /// the same header — S3/MinIO return whatever <c>Content-Encoding</c>
    /// was stored with the object, unconditionally, regardless of what the
    /// GET request itself asked for).
    /// </summary>
    internal static bool HasZstdContentEncoding(HttpResponseMessage response)
    {
        foreach (var value in response.Content.Headers.ContentEncoding)
        {
            if (string.Equals(value, "zstd", StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }
}
