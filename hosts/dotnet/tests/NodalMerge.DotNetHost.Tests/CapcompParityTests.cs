using System.Text.Json;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Asserts this host's <see cref="CapabilityProfileExpander"/> against the
/// canonical CAPCOMP vectors (<c>engine/commands/capcomp-vectors.v1.json</c>,
/// linked into this project as <c>capcomp-vectors.v1.json</c>). The Rust
/// mirror is <c>server/capability-profile/tests/capcomp_vectors.rs</c>, which
/// reads the same file. To change the algorithm, update the vectors, both
/// harnesses, and <c>docs/CAPCOMP_PARITY_PLAN.md</c> together.
///
/// <c>mode: "expand"</c> vectors write their profile to a temp file and run
/// the real <see cref="CapabilityProfileExpander.TryExpand(IReadOnlyList{string}?, string?, out IReadOnlyList{string}, out string?, out string?)"/>
/// path, asserting either the flattened capability list or a stable error
/// class. <c>mode: "passthrough"</c> vectors construct a disabled expander
/// (no profile configured) and assert capabilities pass through unchanged.
/// </summary>
public sealed class CapcompParityTests
{
    private sealed record Vector(
        string Id,
        string Mode,
        JsonElement? Profile,
        IReadOnlyList<string> Assigned,
        string? ClaimedProfileVersion,
        IReadOnlyList<string>? ExpectOk,
        string? ExpectError
    );

    private static readonly IReadOnlyDictionary<string, Vector> VectorsById = LoadVectors();

    private static IReadOnlyDictionary<string, Vector> LoadVectors()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "capcomp-vectors.v1.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var vectors = new Dictionary<string, Vector>(StringComparer.Ordinal);

        foreach (var el in doc.RootElement.GetProperty("vectors").EnumerateArray())
        {
            var id = el.GetProperty("id").GetString()!;
            var mode = el.GetProperty("mode").GetString()!;
            JsonElement? profile = el.TryGetProperty("profile", out var profileEl)
                ? profileEl.Clone()
                : null;
            var assigned = el.GetProperty("assigned")
                .EnumerateArray()
                .Select(x => x.GetString()!)
                .ToList();
            string? claimed = el.TryGetProperty("claimed_profile_version", out var claimedEl)
                && claimedEl.ValueKind != JsonValueKind.Null
                ? claimedEl.GetString()
                : null;

            var expectEl = el.GetProperty("expect");
            List<string>? expectOk = expectEl.TryGetProperty("ok", out var okEl)
                ? okEl.EnumerateArray().Select(x => x.GetString()!).ToList()
                : null;
            string? expectError = expectEl.TryGetProperty("error", out var errEl)
                ? errEl.GetString()
                : null;

            vectors.Add(id, new Vector(id, mode, profile, assigned, claimed, expectOk, expectError));
        }

        return vectors;
    }

    public static TheoryData<string> VectorIds()
    {
        var data = new TheoryData<string>();
        foreach (var id in VectorsById.Keys)
        {
            data.Add(id);
        }
        return data;
    }

    [Fact]
    public void VectorsFileLoadsWithEntries()
    {
        Assert.NotEmpty(VectorsById);
        Assert.Contains("single-node", VectorsById.Keys);
    }

    [Theory]
    [MemberData(nameof(VectorIds))]
    public void VectorMatchesDotNetExpansion(string id)
    {
        var vector = VectorsById[id];

        if (vector.Mode == "passthrough")
        {
            AssertPassthrough(vector);
            return;
        }

        Assert.Equal("expand", vector.Mode);
        AssertExpand(vector);
    }

    private static void AssertPassthrough(Vector vector)
    {
        var options = new CapabilityCompositionOptions(Enabled: false, ProfilePath: null);
        var expander = new CapabilityProfileExpander(options);

        var ok = expander.TryExpand(
            vector.Assigned,
            requestedProfileVersion: null,
            out var expanded,
            out var error,
            out _
        );

        Assert.True(ok, $"vector `{vector.Id}`: expected passthrough success, got error `{error}`");
        Assert.NotNull(vector.ExpectOk);
        Assert.Equal(vector.ExpectOk, expanded);
    }

    private static void AssertExpand(Vector vector)
    {
        Assert.NotNull(vector.Profile);
        using var tempProfile = TempProfile(vector.Profile!.Value.GetRawText());

        var options = new CapabilityCompositionOptions(Enabled: true, ProfilePath: tempProfile.Path);
        options.Validate();
        var expander = new CapabilityProfileExpander(options);

        var ok = expander.TryExpand(
            vector.Assigned,
            vector.ClaimedProfileVersion,
            out var expanded,
            out var error,
            out var errorClass
        );

        if (vector.ExpectOk is not null)
        {
            Assert.True(
                ok,
                $"vector `{vector.Id}`: expected ok {string.Join(",", vector.ExpectOk)}, got error `{error}` (class `{errorClass}`)"
            );
            Assert.Equal(vector.ExpectOk, expanded);
        }
        else
        {
            Assert.NotNull(vector.ExpectError);
            Assert.False(
                ok,
                $"vector `{vector.Id}`: expected error class `{vector.ExpectError}`, got ok [{string.Join(",", expanded)}]"
            );
            Assert.Equal(vector.ExpectError, errorClass);
        }
    }

    private static TempProfileFile TempProfile(string json)
    {
        var path = Path.Combine(Path.GetTempPath(), $"nodalmerge-capcomp-{Guid.NewGuid():N}.json");
        File.WriteAllText(path, json);
        return new TempProfileFile(path);
    }

    private sealed class TempProfileFile : IDisposable
    {
        public TempProfileFile(string path)
        {
            Path = path;
        }

        public string Path { get; }

        public void Dispose()
        {
            if (File.Exists(Path))
            {
                File.Delete(Path);
            }
        }
    }
}
