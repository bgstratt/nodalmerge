using NodalMerge.DotNetHost.Ffi;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;

namespace NodalMerge.DotNetHost.Runtime;

/// <summary>
/// Mirrors inbound WS <c>pack</c> traffic into <see cref="LocalPersistFfiClient"/> (headless-style peer-local log).
/// </summary>
public sealed class RuntimePeerLocalPersistenceService : IDisposable
{
    private readonly LocalPersistFfiClient? _client;
    private readonly ILogger<RuntimePeerLocalPersistenceService> _logger;

    public RuntimePeerLocalPersistenceService(RuntimePeerLocalPersistenceOptions options)
        : this(options, NullLogger<RuntimePeerLocalPersistenceService>.Instance)
    {
    }

    public RuntimePeerLocalPersistenceService(
        RuntimePeerLocalPersistenceOptions options,
        ILogger<RuntimePeerLocalPersistenceService> logger
    )
    {
        _logger = logger;
        if (!options.Enabled)
        {
            _client = null;
            return;
        }

        _client = new LocalPersistFfiClient(options.Backend, options.DataDir);
        _logger.LogInformation(
            "runtime peer-local persistence enabled backend={Backend} durable={Durable} data_dir={DataDir}",
            options.Backend,
            _client.IsDurable,
            options.DataDir ?? "<none>"
        );
    }

    public bool IsEnabled => _client is not null;

    public Task HydrateRoomIfNeededAsync(string roomId, CancellationToken cancellationToken = default)
    {
        if (_client is null || string.IsNullOrWhiteSpace(roomId))
        {
            return Task.CompletedTask;
        }

        cancellationToken.ThrowIfCancellationRequested();
        try
        {
            using var report = _client.Hydrate(roomId);
            _logger.LogDebug(
                "runtime peer-local hydrate room={Room} report={Report}",
                roomId,
                report.RootElement.GetRawText()
            );
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime peer-local hydrate failed room={Room}", roomId);
        }

        return Task.CompletedTask;
    }

    public Task PersistInboundPackAsync(
        string roomId,
        string nodesB64,
        CancellationToken cancellationToken = default
    )
    {
        if (_client is null || string.IsNullOrWhiteSpace(roomId) || string.IsNullOrWhiteSpace(nodesB64))
        {
            return Task.CompletedTask;
        }

        cancellationToken.ThrowIfCancellationRequested();
        try
        {
            using var report = _client.AppendPackB64(roomId, nodesB64);
            _logger.LogDebug(
                "runtime peer-local append pack room={Room} report={Report}",
                roomId,
                report.RootElement.GetRawText()
            );
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime peer-local append pack failed room={Room}", roomId);
        }

        return Task.CompletedTask;
    }

    public Task FlushRoomAsync(string roomId, CancellationToken cancellationToken = default)
    {
        if (_client is null || string.IsNullOrWhiteSpace(roomId))
        {
            return Task.CompletedTask;
        }

        cancellationToken.ThrowIfCancellationRequested();
        try
        {
            using var report = _client.Flush(roomId);
            _logger.LogDebug(
                "runtime peer-local flush room={Room} report={Report}",
                roomId,
                report.RootElement.GetRawText()
            );
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime peer-local flush failed room={Room}", roomId);
        }

        return Task.CompletedTask;
    }

    public void Dispose()
    {
        _client?.Dispose();
    }
}
