namespace ActiveSync.Host.Abstractions.Providers;

public interface IProviderHealthCheck
{
    ValueTask<ProviderHealthStatus> CheckAsync(CancellationToken cancellationToken = default);
}

public sealed record ProviderHealthStatus(string Name, bool IsHealthy, string? Details = null);
