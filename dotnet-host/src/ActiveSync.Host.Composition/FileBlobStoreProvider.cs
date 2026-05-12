using ActiveSync.Host.Abstractions.Providers;

namespace ActiveSync.Host.Composition;

internal sealed class FileBlobStoreProvider : IBlobStoreProvider, IBlobUrlResolverProvider
{
    private readonly string _rootPath;

    public FileBlobStoreProvider(FileBlobStorageOptions options)
    {
        _rootPath = Path.GetFullPath(options.RootPath);
        Directory.CreateDirectory(_rootPath);
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        var path = GetPath(hashHex);
        if (!File.Exists(path))
        {
            return BlobReadResult.Missing;
        }

        var bytes = await File.ReadAllBytesAsync(path, cancellationToken);
        return BlobReadResult.Hit(bytes, contentType: null);
    }

    public async ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        var path = GetPath(hashHex);
        var parent = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(parent))
        {
            Directory.CreateDirectory(parent);
        }

        await File.WriteAllBytesAsync(path, bytes, cancellationToken);
    }

    public ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
        BlobPutUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult<PresignedBlobUrl?>(null);
    }

    public ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
        BlobGetUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult<PresignedBlobUrl?>(null);
    }

    private string GetPath(string hashHex)
    {
        var safe = hashHex
            .Replace(':', '_')
            .Replace('/', '_')
            .Replace('\\', '_');

        var shard = safe.Length >= 2 ? safe[..2] : "00";
        return Path.Combine(_rootPath, shard, safe + ".blob");
    }
}
