using ActiveSync.Host.Composition;

namespace ActiveSync.DotNetHost.Tests;

public sealed class CapabilityProfileExpanderTests
{
    [Fact]
    public void TryExpand_flattens_and_sorts_when_enabled()
    {
        using var profile = TempProfile(
            """
            {
              "profile_version": "capprof-v1",
              "nodes": [
                { "capability": "room.admin", "inherits": ["tick.admin", "policy.admin"] },
                { "capability": "tick.admin", "inherits": [] },
                { "capability": "policy.admin", "inherits": [] }
              ]
            }
            """
        );

        var options = new CapabilityCompositionOptions(
            Enabled: true,
            ProfilePath: profile.Path
        );
        options.Validate();

        var expander = new CapabilityProfileExpander(options);

        var ok = expander.TryExpand(
            assignedCapabilities: [" room.admin "],
            requestedProfileVersion: "capprof-v1",
            out var expanded,
            out var error
        );

        Assert.True(ok);
        Assert.Null(error);
        Assert.Equal(["policy.admin", "room.admin", "tick.admin"], expanded);
    }

    [Fact]
    public void TryExpand_rejects_unknown_profile_version_when_enabled()
    {
        using var profile = TempProfile(
            """
            {
              "profile_version": "capprof-v1",
              "nodes": [
                { "capability": "policy.admin", "inherits": [] }
              ]
            }
            """
        );

        var options = new CapabilityCompositionOptions(
            Enabled: true,
            ProfilePath: profile.Path
        );
        options.Validate();

        var expander = new CapabilityProfileExpander(options);

        var ok = expander.TryExpand(
            assignedCapabilities: ["policy.admin"],
            requestedProfileVersion: "capprof-v9",
            out var expanded,
            out var error
        );

        Assert.False(ok);
        Assert.NotNull(error);
        Assert.Equal("unknown capability_profile_version 'capprof-v9'", error);
        Assert.Single(expanded);
    }

    [Fact]
    public void TryExpand_allows_supported_profile_version_compatibility_window()
    {
        using var profile = TempProfile(
            """
            {
              "profile_version": "capprof-v3",
              "supported_profile_versions": ["capprof-v2"],
              "nodes": [
                { "capability": "policy.admin", "inherits": [] }
              ]
            }
            """
        );

        var options = new CapabilityCompositionOptions(
            Enabled: true,
            ProfilePath: profile.Path
        );
        options.Validate();

        var expander = new CapabilityProfileExpander(options);

        var ok = expander.TryExpand(
            assignedCapabilities: ["policy.admin"],
            requestedProfileVersion: "capprof-v2",
            out var expanded,
            out var error
        );

        Assert.True(ok);
        Assert.Null(error);
        Assert.Equal(["policy.admin"], expanded);
    }

        [Fact]
        public void TryExpand_rejects_unknown_inheritance_reference()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "nodes": [
                                { "capability": "compose.root", "inherits": ["compose.missing"] },
                                { "capability": "compose.leaf", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.Equal("unknown inheritance reference 'compose.missing' (from 'compose.root')", error);
        }

        [Fact]
        public void TryExpand_rejects_duplicate_capability_node()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "nodes": [
                                { "capability": "compose.root", "inherits": [] },
                                { "capability": "compose.root", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.Equal("duplicate capability node 'compose.root'", error);
        }

        [Fact]
        public void TryExpand_rejects_edges_limit_overflow()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "limits": {
                                "max_capability_count": 128,
                                "max_capability_length": 128,
                                "max_flattened_payload_bytes": 8192,
                                "max_dag_depth": 16,
                                "max_edges_per_node": 1
                            },
                            "nodes": [
                                { "capability": "compose.root", "inherits": ["compose.a", "compose.b"] },
                                { "capability": "compose.a", "inherits": [] },
                                { "capability": "compose.b", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.Equal("capability 'compose.root' exceeds max edges per node (1)", error);
        }

        [Fact]
        public void TryExpand_rejects_payload_size_limit_overflow()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "limits": {
                                "max_capability_count": 128,
                                "max_capability_length": 128,
                                "max_flattened_payload_bytes": 12,
                                "max_dag_depth": 16,
                                "max_edges_per_node": 64
                            },
                            "nodes": [
                                { "capability": "compose.root", "inherits": ["compose.alpha"] },
                                { "capability": "compose.alpha", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.Equal("flattened capability payload exceeded max bytes (12)", error);
        }

        [Fact]
        public void Ctor_rejects_empty_supported_profile_versions_entry()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v3",
                            "supported_profile_versions": ["capprof-v2", ""],
                            "nodes": [
                                { "capability": "policy.admin", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var ex = Assert.Throws<InvalidOperationException>(() => new CapabilityProfileExpander(options));
                Assert.Equal("Capability profile supported_profile_versions entries must not be empty", ex.Message);
        }

        [Fact]
        public void TryExpand_deduplicates_multi_path_inheritance()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "nodes": [
                                { "capability": "compose.root", "inherits": ["compose.a", "compose.b"] },
                                { "capability": "compose.a", "inherits": ["compose.shared"] },
                                { "capability": "compose.b", "inherits": ["compose.shared"] },
                                { "capability": "compose.shared", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out var expanded,
                        out var error
                );

                Assert.True(ok);
                Assert.Null(error);
                Assert.Equal(["compose.a", "compose.b", "compose.root", "compose.shared"], expanded);
        }

        [Fact]
        public void TryExpand_rejects_cycle_graph()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "nodes": [
                                { "capability": "compose.a", "inherits": ["compose.b"] },
                                { "capability": "compose.b", "inherits": ["compose.a"] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.a"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.NotNull(error);
                Assert.Equal("cycle detected at capability 'compose.a'", error);
        }

        [Fact]
        public void TryExpand_rejects_count_limit_overflow()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "limits": {
                                "max_capability_count": 2,
                                "max_capability_length": 128,
                                "max_flattened_payload_bytes": 8192,
                                "max_dag_depth": 16,
                                "max_edges_per_node": 64
                            },
                            "nodes": [
                                { "capability": "compose.root", "inherits": ["compose.a", "compose.b"] },
                                { "capability": "compose.a", "inherits": [] },
                                { "capability": "compose.b", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.NotNull(error);
                Assert.Equal("flattened capability count exceeded max (2)", error);
        }

            [Fact]
            public void TryExpand_rejects_depth_limit_overflow()
            {
                using var profile = TempProfile(
                    """
                    {
                        "profile_version": "capprof-v1",
                        "limits": {
                        "max_capability_count": 128,
                        "max_capability_length": 128,
                        "max_flattened_payload_bytes": 8192,
                        "max_dag_depth": 1,
                        "max_edges_per_node": 64
                        },
                        "nodes": [
                        { "capability": "compose.root", "inherits": ["compose.a"] },
                        { "capability": "compose.a", "inherits": ["compose.b"] },
                        { "capability": "compose.b", "inherits": [] }
                        ]
                    }
                    """
                );

                var options = new CapabilityCompositionOptions(
                    Enabled: true,
                    ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                    assignedCapabilities: ["compose.root"],
                    requestedProfileVersion: "capprof-v1",
                    out _,
                    out var error
                );

                Assert.False(ok);
                Assert.NotNull(error);
                Assert.Equal("capability graph depth exceeded max (1)", error);
            }

        [Fact]
        public void TryExpand_rejects_invalid_capability_grammar()
        {
                using var profile = TempProfile(
                        """
                        {
                            "profile_version": "capprof-v1",
                            "nodes": [
                                { "capability": "compose.root", "inherits": ["policy.*"] },
                                { "capability": "policy.admin", "inherits": [] }
                            ]
                        }
                        """
                );

                var options = new CapabilityCompositionOptions(
                        Enabled: true,
                        ProfilePath: profile.Path
                );
                options.Validate();

                var expander = new CapabilityProfileExpander(options);

                var ok = expander.TryExpand(
                        assignedCapabilities: ["compose.root"],
                        requestedProfileVersion: "capprof-v1",
                        out _,
                        out var error
                );

                Assert.False(ok);
                Assert.NotNull(error);
                Assert.Equal("invalid capability token 'policy.*'", error);
        }

    private static TempProfileFile TempProfile(string json)
    {
        var path = Path.Combine(Path.GetTempPath(), $"activesync-capprof-{Guid.NewGuid():N}.json");
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
