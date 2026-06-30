using System.Text.Json;
using System.Text.Json.Serialization;

namespace NodalMerge.Host.Composition;

public sealed class CapabilityProfileExpander
{
    private const int DefaultMaxCapabilityCount = 128;
    private const int DefaultMaxCapabilityLength = 128;
    private const int DefaultMaxFlattenedPayloadBytes = 8 * 1024;
    private const int DefaultMaxDagDepth = 16;
    private const int DefaultMaxEdgesPerNode = 64;

    private readonly CapabilityCompositionOptions _options;
    private readonly CapabilityProfileDocument? _profile;

    public CapabilityProfileExpander(CapabilityCompositionOptions options)
    {
        _options = options;

        if (!_options.Enabled)
        {
            return;
        }

        var path = _options.ProfilePath!;
        var raw = File.ReadAllText(path);
        _profile = JsonSerializer.Deserialize<CapabilityProfileDocument>(raw)
            ?? throw new InvalidOperationException($"Capability profile JSON is empty or invalid: {path}");

        if (string.IsNullOrWhiteSpace(_profile.ProfileVersion))
        {
            throw new InvalidOperationException("Capability profile requires non-empty profile_version");
        }

        foreach (var version in _profile.SupportedProfileVersions)
        {
            if (string.IsNullOrWhiteSpace(version))
            {
                throw new InvalidOperationException("Capability profile supported_profile_versions entries must not be empty");
            }
        }

        if (_profile.Nodes is null || _profile.Nodes.Count == 0)
        {
            throw new InvalidOperationException("Capability profile requires at least one node");
        }
    }

    public bool IsEnabled => _options.Enabled;

    public bool TryExpand(
        IReadOnlyList<string>? assignedCapabilities,
        string? requestedProfileVersion,
        out IReadOnlyList<string> expandedCapabilities,
        out string? error)
    {
        expandedCapabilities = assignedCapabilities ?? [];
        error = null;

        if (!IsEnabled)
        {
            return true;
        }

        var profile = _profile!;
        var limits = profile.Limits ?? new CapabilityProfileLimits();

        var requested = requestedProfileVersion?.Trim();
        if (string.IsNullOrWhiteSpace(requested))
        {
            error = "missing capability_profile_version while capability composition is enabled";
            return false;
        }

        if (!SupportsProfileVersion(profile, requested))
        {
            error = $"unknown capability_profile_version '{requested}'";
            return false;
        }

        var graph = new Dictionary<string, List<string>>(StringComparer.Ordinal);

        foreach (var node in profile.Nodes)
        {
            var cap = NormalizeToken(node.Capability, limits.MaxCapabilityLength, out var capErr);
            if (capErr is not null)
            {
                error = capErr;
                return false;
            }

            if (graph.ContainsKey(cap!))
            {
                error = $"duplicate capability node '{cap}'";
                return false;
            }

            var inherits = new List<string>();
            foreach (var parentRaw in node.Inherits ?? [])
            {
                var parent = NormalizeToken(parentRaw, limits.MaxCapabilityLength, out var parentErr);
                if (parentErr is not null)
                {
                    error = parentErr;
                    return false;
                }
                inherits.Add(parent!);
            }

            if (inherits.Count > limits.MaxEdgesPerNode)
            {
                error = $"capability '{cap}' exceeds max edges per node ({limits.MaxEdgesPerNode})";
                return false;
            }

            graph[cap!] = inherits;
        }

        foreach (var (node, parents) in graph)
        {
            foreach (var parent in parents)
            {
                if (!graph.ContainsKey(parent))
                {
                    error = $"unknown inheritance reference '{parent}' (from '{node}')";
                    return false;
                }
            }
        }

        var expanded = new SortedSet<string>(StringComparer.Ordinal);
        var visiting = new HashSet<string>(StringComparer.Ordinal);

        foreach (var raw in assignedCapabilities ?? [])
        {
            var source = NormalizeToken(raw, limits.MaxCapabilityLength, out var sourceErr);
            if (sourceErr is not null)
            {
                error = sourceErr;
                return false;
            }

            if (!graph.ContainsKey(source!))
            {
                error = $"unknown assigned capability '{source}'";
                return false;
            }

            if (!TryExpandDfs(source!, 0, graph, visiting, expanded, limits, out error))
            {
                return false;
            }
        }

        if (expanded.Count > limits.MaxCapabilityCount)
        {
            error = $"flattened capability count exceeded max ({limits.MaxCapabilityCount})";
            return false;
        }

        var payloadBytes = string.Join(",", expanded).Length;
        if (payloadBytes > limits.MaxFlattenedPayloadBytes)
        {
            error = $"flattened capability payload exceeded max bytes ({limits.MaxFlattenedPayloadBytes})";
            return false;
        }

        expandedCapabilities = expanded.ToArray();
        return true;
    }

    private static bool TryExpandDfs(
        string cap,
        int depth,
        IReadOnlyDictionary<string, List<string>> graph,
        HashSet<string> visiting,
        SortedSet<string> expanded,
        CapabilityProfileLimits limits,
        out string? error)
    {
        error = null;

        if (depth > limits.MaxDagDepth)
        {
            error = $"capability graph depth exceeded max ({limits.MaxDagDepth})";
            return false;
        }

        if (visiting.Contains(cap))
        {
            error = $"cycle detected at capability '{cap}'";
            return false;
        }

        if (expanded.Contains(cap))
        {
            return true;
        }

        visiting.Add(cap);

        if (graph.TryGetValue(cap, out var parents))
        {
            foreach (var parent in parents)
            {
                if (!TryExpandDfs(parent, depth + 1, graph, visiting, expanded, limits, out error))
                {
                    return false;
                }
            }
        }

        visiting.Remove(cap);
        expanded.Add(cap);
        return true;
    }

    private static string? NormalizeToken(string? raw, int maxLen, out string? error)
    {
        error = null;
        if (string.IsNullOrWhiteSpace(raw))
        {
            error = "capability token must not be empty";
            return null;
        }

        var token = raw.Trim().ToLowerInvariant();
        if (token.Length > maxLen)
        {
            error = $"capability token exceeds max length ({maxLen})";
            return null;
        }

        var first = token[0];
        if (!(first is >= 'a' and <= 'z') && !char.IsAsciiDigit(first))
        {
            error = $"invalid capability token '{raw}'";
            return null;
        }

        foreach (var c in token)
        {
            if ((c is >= 'a' and <= 'z') || char.IsAsciiDigit(c) || c is '.' or '_' or '-')
            {
                continue;
            }

            error = $"invalid capability token '{raw}'";
            return null;
        }

        return token;
    }

    private static bool SupportsProfileVersion(CapabilityProfileDocument profile, string requested)
    {
        if (string.Equals(requested, profile.ProfileVersion, StringComparison.Ordinal))
        {
            return true;
        }

        foreach (var version in profile.SupportedProfileVersions)
        {
            if (string.Equals(requested, version?.Trim(), StringComparison.Ordinal))
            {
                return true;
            }
        }

        return false;
    }

    private sealed class CapabilityProfileDocument
    {
        [JsonPropertyName("profile_version")]
        public string ProfileVersion { get; set; } = string.Empty;

        [JsonPropertyName("nodes")]
        public List<CapabilityProfileNode> Nodes { get; set; } = [];

        [JsonPropertyName("supported_profile_versions")]
        public List<string> SupportedProfileVersions { get; set; } = [];

        [JsonPropertyName("limits")]
        public CapabilityProfileLimits? Limits { get; set; }
    }

    private sealed class CapabilityProfileNode
    {
        [JsonPropertyName("capability")]
        public string Capability { get; set; } = string.Empty;

        [JsonPropertyName("inherits")]
        public List<string>? Inherits { get; set; }
    }

    private sealed class CapabilityProfileLimits
    {
        [JsonPropertyName("max_capability_count")]
        public int MaxCapabilityCount { get; set; } = DefaultMaxCapabilityCount;

        [JsonPropertyName("max_capability_length")]
        public int MaxCapabilityLength { get; set; } = DefaultMaxCapabilityLength;

        [JsonPropertyName("max_flattened_payload_bytes")]
        public int MaxFlattenedPayloadBytes { get; set; } = DefaultMaxFlattenedPayloadBytes;

        [JsonPropertyName("max_dag_depth")]
        public int MaxDagDepth { get; set; } = DefaultMaxDagDepth;

        [JsonPropertyName("max_edges_per_node")]
        public int MaxEdgesPerNode { get; set; } = DefaultMaxEdgesPerNode;
    }
}
