namespace NodalMerge.Host.Abstractions.Providers;

/// <summary>
/// Push seam for the reconcile sweep: given a blob the local store already
/// holds, push it to a remote blob origin and cheaply check whether the
/// origin already has it. Implemented by remote blob origins (e.g.
/// <c>HttpRemoteBlobStoreProvider</c>) so the sweep can heal an origin that
/// missed a write without depending on the full <see cref="IBlobStoreProvider"/>
/// surface (which is chain-shaped and answers reads from the local cache
/// first).
/// </summary>
public interface IRemoteBlobPushTarget
{
    ValueTask<bool> ExistsAsync(string hashHex, CancellationToken ct = default);

    ValueTask PushAsync(string hashHex, byte[] bytes, string? contentType, CancellationToken ct = default);
}
