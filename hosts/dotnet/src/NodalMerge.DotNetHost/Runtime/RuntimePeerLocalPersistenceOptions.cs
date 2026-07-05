namespace NodalMerge.DotNetHost.Runtime;

/// <summary>
/// Optional in-process peer-local persistence for the runtime WebSocket path.
/// </summary>
public sealed class RuntimePeerLocalPersistenceOptions
{
    public const string SectionName = "NodalMerge:Runtime:PeerLocal";

    public bool Enabled { get; init; }

    public string Backend { get; init; } = "memory";

    public string? DataDir { get; init; }

    public static RuntimePeerLocalPersistenceOptions Disabled { get; } = new() { Enabled = false };
}
