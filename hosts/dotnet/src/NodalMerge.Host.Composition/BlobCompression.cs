using ZstdSharp;

namespace NodalMerge.Host.Composition;

/// <summary>
/// zstd at-rest encoding helpers for <see cref="FileBlobStoreProvider"/>
/// (docs/BLOB_STORAGE_LAYOUT.md §8). A blob's identity is always Blake3 of
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
    /// </summary>
    internal static bool ShouldCompress(byte[] bytes, string? contentType, FileBlobStorageOptions options)
    {
        if (bytes.Length < options.CompressionMinBytes)
        {
            return false;
        }

        if (IsCompressedMediaContentType(contentType))
        {
            return false;
        }

        var sampleLength = Math.Min(SampleSize, bytes.Length);
        byte[] compressedSample;
        using (var compressor = new Compressor(options.CompressionLevel))
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
}
