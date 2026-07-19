using System.Diagnostics;
using System.Text;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Hard-ceiling clamp on profile-supplied <c>limits</c>
/// (plans/blob-cas-remediation.md §4.4, finding #15). The built-in defaults
/// (128 caps / 128-char tokens / 8 KiB payload / depth 16 / 64 edges) are
/// ceilings, not just fallbacks: a profile may lower a limit but never raise
/// it, so a hostile or fat-fingered profile file cannot lift the RoomToken
/// mint/validate guard (<c>RoomTokenEmbeddedAuth</c>) or unbound the DFS.
/// Rust mirror: <c>server/capability-profile/tests/limits_clamp.rs</c>;
/// cross-runtime lock: the <c>limits-above-ceiling-*</c> CAPCOMP vectors.
/// </summary>
public sealed class CapabilityProfileLimitsClampTests
{
    private const int CeilingCount = 128;
    private const int CeilingLength = 128;
    private const int CeilingPayloadBytes = 8 * 1024;
    private const int CeilingDepth = 16;
    private const int CeilingEdges = 64;

    // ------------------------------------------------------------------
    // RED: oversized `limits` must be clamped to the historical ceilings
    // (pre-fix each of these expands successfully — the raised limit is
    // honored; post-fix the same input is rejected at the ceiling).
    // ------------------------------------------------------------------

    [Fact]
    public void Oversized_count_limit_is_clamped_to_historical_ceiling()
    {
        // 130 flattened caps (root -> 2 mids -> 127 leaves), profile claims 1024.
        using var profile = TempProfile(LayeredProfileJson(totalCaps: CeilingCount + 2, limitsJson: "\"max_capability_count\": 1024"));
        var expander = Expander(profile.Path);

        var ok = expander.TryExpand(["root"], "v1", out _, out var error, out var errorClass);

        Assert.False(ok, "a 130-cap expansion must not out-mint the built-in 128-cap ceiling");
        Assert.Equal("count_exceeded", errorClass);
        Assert.Contains($"({CeilingCount})", error);
    }

    [Fact]
    public void Oversized_depth_limit_keeps_dfs_bounded_at_historical_ceiling()
    {
        // An 18-node chain reaches DFS depth 17 — one past the historical
        // bound of 16 — while the profile claims depth 64 is fine.
        using var profile = TempProfile(ChainProfileJson(chainLength: CeilingDepth + 2, limitsJson: "\"max_dag_depth\": 64"));
        var expander = Expander(profile.Path);

        var ok = expander.TryExpand(["c0000"], "v1", out _, out var error, out var errorClass);

        Assert.False(ok, "the DFS must stop at the built-in depth ceiling, not the profile's 64");
        Assert.Equal("depth_exceeded", errorClass);
        Assert.Contains($"({CeilingDepth})", error);
    }

    [Fact]
    public void Oversized_edges_limit_is_clamped_to_historical_ceiling()
    {
        // 65 direct edges on one node, profile claims 512 are allowed.
        using var profile = TempProfile(FanOutProfileJson(leafCount: CeilingEdges + 1, limitsJson: "\"max_edges_per_node\": 512"));
        var expander = Expander(profile.Path);

        var ok = expander.TryExpand(["root"], "v1", out _, out var error, out var errorClass);

        Assert.False(ok, "65 edges on one node must exceed the built-in 64-edge ceiling");
        Assert.Equal("too_many_edges", errorClass);
        Assert.Contains($"({CeilingEdges})", error);
    }

    [Fact]
    public void Oversized_payload_limit_is_clamped_to_historical_ceiling()
    {
        // 73 caps of 120 chars ≈ 8.6 KiB comma-joined — over the historical
        // 8 KiB, under the profile's claimed 1 MB, and inside the count,
        // edge and length caps so only the payload cap can fire.
        using var profile = TempProfile(WidePayloadProfileJson(limitsJson: "\"max_flattened_payload_bytes\": 1000000"));
        var expander = Expander(profile.Path);

        var ok = expander.TryExpand([PaddedName("rt", 0)], "v1", out _, out var error, out var errorClass);

        Assert.False(ok, "an 8.6 KiB payload must exceed the built-in 8 KiB ceiling");
        Assert.Equal("payload_exceeded", errorClass);
        Assert.Contains($"({CeilingPayloadBytes})", error);
    }

    [Fact]
    public void Oversized_length_limit_is_clamped_to_historical_ceiling()
    {
        // A 129-char token, profile claims 256 chars are allowed.
        var longToken = "a" + new string('b', CeilingLength);
        using var profile = TempProfile($$"""
            {
              "profile_version": "v1",
              "limits": { "max_capability_length": 256 },
              "nodes": [ { "capability": "{{longToken}}", "inherits": [] } ]
            }
            """);
        var expander = Expander(profile.Path);

        var ok = expander.TryExpand([longToken], "v1", out _, out _, out var errorClass);

        Assert.False(ok, "a 129-char token must exceed the built-in 128-char ceiling");
        Assert.Equal("invalid_token", errorClass);
    }

    // ------------------------------------------------------------------
    // Clamp warning (RED pre-fix: nothing is emitted at all).
    // ------------------------------------------------------------------

    [Fact]
    public void Clamp_warns_with_field_requested_value_and_ceiling()
    {
        // Sentinel 31337 keeps this assertion immune to trace noise from
        // tests running in parallel collections.
        using var profileFile = TempProfile("""
            {
              "profile_version": "v1",
              "limits": { "max_capability_count": 31337 },
              "nodes": [ { "capability": "room.member", "inherits": [] } ]
            }
            """);

        var listener = new CapturingTraceListener();
        Trace.Listeners.Add(listener);
        try
        {
            _ = Expander(profileFile.Path);
        }
        finally
        {
            Trace.Listeners.Remove(listener);
        }

        var warn = Assert.Single(listener.Messages, m => m.Contains("31337"));
        Assert.Contains("max_capability_count", warn);
        Assert.Contains(CeilingCount.ToString(), warn);
    }

    [Fact]
    public void Clamp_warn_does_not_fire_for_lowered_or_absent_limits()
    {
        // No test anywhere clamps max_edges_per_node, so filtering on the
        // field name isolates this assertion from parallel-collection trace
        // noise. Lowering to 63 must stay silent (and enforced).
        using var noLimits = TempProfile("""
            {
              "profile_version": "v1",
              "nodes": [ { "capability": "room.member", "inherits": [] } ]
            }
            """);
        using var lowered = TempProfile("""
            {
              "profile_version": "v1",
              "limits": { "max_edges_per_node": 63 },
              "nodes": [ { "capability": "room.member", "inherits": [] } ]
            }
            """);

        var listener = new CapturingTraceListener();
        Trace.Listeners.Add(listener);
        try
        {
            _ = Expander(noLimits.Path);
            _ = Expander(lowered.Path);
        }
        finally
        {
            Trace.Listeners.Remove(listener);
        }

        Assert.DoesNotContain(listener.Messages, m => m.Contains("max_edges_per_node"));
    }

    // ------------------------------------------------------------------
    // Green-side equivalence pins: legit profiles are provably unchanged.
    // ------------------------------------------------------------------

    [Fact]
    public void No_limits_profile_enforces_exactly_the_historical_constants()
    {
        // 128 flattened caps pass; 129 fail.
        using var atCap = TempProfile(LayeredProfileJson(totalCaps: CeilingCount, limitsJson: null));
        var okAt = Expander(atCap.Path).TryExpand(["root"], "v1", out var expanded, out var atErr, out _);
        Assert.True(okAt, $"128 caps must pass with no limits block, got: {atErr}");
        Assert.Equal(CeilingCount, expanded.Count);

        using var overCap = TempProfile(LayeredProfileJson(totalCaps: CeilingCount + 1, limitsJson: null));
        var okOver = Expander(overCap.Path).TryExpand(["root"], "v1", out _, out _, out var overClass);
        Assert.False(okOver, "129 caps must fail with no limits block");
        Assert.Equal("count_exceeded", overClass);

        // Depth 16 passes; depth 17 fails.
        using var atDepth = TempProfile(ChainProfileJson(chainLength: CeilingDepth + 1, limitsJson: null));
        Assert.True(Expander(atDepth.Path).TryExpand(["c0000"], "v1", out _, out _, out _));

        using var overDepth = TempProfile(ChainProfileJson(chainLength: CeilingDepth + 2, limitsJson: null));
        var okDeep = Expander(overDepth.Path).TryExpand(["c0000"], "v1", out _, out _, out var deepClass);
        Assert.False(okDeep, "depth 17 must fail with no limits block");
        Assert.Equal("depth_exceeded", deepClass);
    }

    [Fact]
    public void Limits_at_the_ceilings_behave_identically_to_no_limits()
    {
        const string atCeilings =
            "\"max_capability_count\": 128, \"max_capability_length\": 128, \"max_flattened_payload_bytes\": 8192, \"max_dag_depth\": 16, \"max_edges_per_node\": 64";

        using var overDepth = TempProfile(ChainProfileJson(chainLength: CeilingDepth + 2, limitsJson: atCeilings));
        var okDeep = Expander(overDepth.Path).TryExpand(["c0000"], "v1", out _, out _, out var deepClass);
        Assert.False(okDeep, "depth 17 must fail with limits at the ceilings");
        Assert.Equal("depth_exceeded", deepClass);

        using var atDepth = TempProfile(ChainProfileJson(chainLength: CeilingDepth + 1, limitsJson: atCeilings));
        Assert.True(Expander(atDepth.Path).TryExpand(["c0000"], "v1", out _, out _, out _));
    }

    [Fact]
    public void Lowered_limits_still_lower()
    {
        using var profile = TempProfile(ChainProfileJson(chainLength: 4, limitsJson: "\"max_dag_depth\": 2"));
        var ok = Expander(profile.Path).TryExpand(["c0000"], "v1", out _, out var error, out var errorClass);

        Assert.False(ok, "a lowered depth limit must still be enforced");
        Assert.Equal("depth_exceeded", errorClass);
        Assert.Contains("(2)", error);
    }

    // ------------------------------------------------------------------
    // Profile builders (kept legal for every cap except the one under test).
    // ------------------------------------------------------------------

    private static CapabilityProfileExpander Expander(string profilePath)
    {
        var options = new CapabilityCompositionOptions(Enabled: true, ProfilePath: profilePath);
        options.Validate();
        return new CapabilityProfileExpander(options);
    }

    /// <summary>
    /// `totalCaps` flattened capabilities (root -> 2 mids -> leaves) with
    /// every node at or under 64 edges and depth 2, so only the *count* cap
    /// can fire.
    /// </summary>
    private static string LayeredProfileJson(int totalCaps, string? limitsJson)
    {
        var leafCount = totalCaps - 3;
        var leaves = Enumerable.Range(0, leafCount).Select(i => $"leaf{i:D4}").ToList();
        var first = leaves.Take(Math.Min(leafCount, 64)).ToList();
        var second = leaves.Skip(first.Count).ToList();

        var nodes = new List<string>
        {
            """{ "capability": "root", "inherits": ["mid0", "mid1"] }""",
            $$"""{ "capability": "mid0", "inherits": [{{string.Join(", ", first.Select(l => $"\"{l}\""))}}] }""",
            $$"""{ "capability": "mid1", "inherits": [{{string.Join(", ", second.Select(l => $"\"{l}\""))}}] }""",
        };
        nodes.AddRange(leaves.Select(l => $$"""{ "capability": "{{l}}", "inherits": [] }"""));

        return ProfileJson(nodes, limitsJson);
    }

    /// <summary>A linear chain: the deepest node sits at DFS depth `chainLength - 1`.</summary>
    private static string ChainProfileJson(int chainLength, string? limitsJson)
    {
        var nodes = Enumerable.Range(0, chainLength)
            .Select(i => i + 1 < chainLength
                ? $$"""{ "capability": "c{{i:D4}}", "inherits": ["c{{i + 1:D4}}"] }"""
                : $$"""{ "capability": "c{{i:D4}}", "inherits": [] }""")
            .ToList();
        return ProfileJson(nodes, limitsJson);
    }

    /// <summary>`root` with `leafCount` direct edges — only for edge-cap tests.</summary>
    private static string FanOutProfileJson(int leafCount, string? limitsJson)
    {
        var leaves = Enumerable.Range(0, leafCount).Select(i => $"leaf{i:D4}").ToList();
        var nodes = new List<string>
        {
            $$"""{ "capability": "root", "inherits": [{{string.Join(", ", leaves.Select(l => $"\"{l}\""))}}] }""",
        };
        nodes.AddRange(leaves.Select(l => $$"""{ "capability": "{{l}}", "inherits": [] }"""));
        return ProfileJson(nodes, limitsJson);
    }

    private static string PaddedName(string prefix, int i) =>
        $"{prefix}{i:D3}{new string('x', 120 - prefix.Length - 3)}";

    /// <summary>73 caps of 120 chars each: comma-joined payload ≈ 8.6 KiB.</summary>
    private static string WidePayloadProfileJson(string? limitsJson)
    {
        var left = Enumerable.Range(0, 35).Select(i => PaddedName("pl", i)).ToList();
        var right = Enumerable.Range(0, 35).Select(i => PaddedName("pr", i)).ToList();
        var mid0 = PaddedName("m0", 0);
        var mid1 = PaddedName("m1", 0);

        var nodes = new List<string>
        {
            $$"""{ "capability": "{{PaddedName("rt", 0)}}", "inherits": ["{{mid0}}", "{{mid1}}"] }""",
            $$"""{ "capability": "{{mid0}}", "inherits": [{{string.Join(", ", left.Select(l => $"\"{l}\""))}}] }""",
            $$"""{ "capability": "{{mid1}}", "inherits": [{{string.Join(", ", right.Select(l => $"\"{l}\""))}}] }""",
        };
        nodes.AddRange(left.Concat(right).Select(l => $$"""{ "capability": "{{l}}", "inherits": [] }"""));
        return ProfileJson(nodes, limitsJson);
    }

    private static string ProfileJson(IReadOnlyList<string> nodes, string? limitsJson)
    {
        var sb = new StringBuilder();
        sb.AppendLine("{");
        sb.AppendLine("  \"profile_version\": \"v1\",");
        if (limitsJson is not null)
        {
            sb.AppendLine($"  \"limits\": {{ {limitsJson} }},");
        }
        sb.AppendLine("  \"nodes\": [");
        sb.AppendLine("    " + string.Join(",\n    ", nodes));
        sb.AppendLine("  ]");
        sb.AppendLine("}");
        return sb.ToString();
    }

    private static TempProfileFile TempProfile(string json)
    {
        var path = Path.Combine(Path.GetTempPath(), $"nodalmerge-limits-clamp-{Guid.NewGuid():N}.json");
        File.WriteAllText(path, json);
        return new TempProfileFile(path);
    }

    private sealed class TempProfileFile(string path) : IDisposable
    {
        public string Path { get; } = path;

        public void Dispose()
        {
            if (File.Exists(Path))
            {
                File.Delete(Path);
            }
        }
    }

    private sealed class CapturingTraceListener : TraceListener
    {
        private readonly List<string> _messages = [];

        public IReadOnlyList<string> Messages
        {
            get
            {
                lock (_messages)
                {
                    return _messages.ToArray();
                }
            }
        }

        public override void Write(string? message)
        {
            if (message is null)
            {
                return;
            }

            lock (_messages)
            {
                _messages.Add(message);
            }
        }

        public override void WriteLine(string? message) => Write(message);
    }
}
