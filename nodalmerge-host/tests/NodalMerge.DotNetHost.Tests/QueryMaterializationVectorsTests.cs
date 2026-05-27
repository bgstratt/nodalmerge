using NodalMerge.DotNetHost.Runtime;

namespace NodalMerge.DotNetHost.Tests;

public sealed class QueryMaterializationVectorsTests
{
    [Fact]
    public void QueryDet002DeterministicDigestEquality()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var register = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(register.DispatchSucceeded);

        var build = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\"}",
            state
        );
        Assert.True(build.DispatchSucceeded);

        var page1 = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":1}",
            state
        );
        var page2 = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":2,\"page_token\":\"offset:1\"}",
            state
        );
        Assert.True(page1.DispatchSucceeded);
        Assert.True(page2.DispatchSucceeded);

        Assert.Contains("\"type\":\"projection.read.result\"", page1.OutboundMessages[0]);
        Assert.Contains("\"type\":\"projection.read.result\"", page2.OutboundMessages[0]);

        var digest1 = ExtractDigest(page1.OutboundMessages[0]);
        var digest2 = ExtractDigest(page2.OutboundMessages[0]);
        Assert.Equal(digest1, digest2);
        Assert.False(string.IsNullOrWhiteSpace(digest1));

        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void QueryInval001InvalidationReasonPropagationParity()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var register = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(register.DispatchSucceeded);

        var build = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\"}",
            state
        );
        Assert.True(build.DispatchSucceeded);

        var invalidate = processor.ProcessIncomingText(
            "{\"type\":\"projection.invalidate\",\"projection_id\":\"p.rooms\",\"reason\":\"schema-change\"}",
            state
        );
        Assert.True(invalidate.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.invalidated\"", invalidate.OutboundMessages[0]);
        Assert.Contains("\"reason\":\"schema-change\"", invalidate.OutboundMessages[0]);

        var listInvalidated = processor.ProcessIncomingText(
            "{\"type\":\"projection.list\",\"query_spec_id\":\"q.rooms\",\"state_filter\":\"invalidated\"}",
            state
        );
        Assert.True(listInvalidated.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.list.result\"", listInvalidated.OutboundMessages[0]);
        Assert.Contains("\"state\":\"invalidated\"", listInvalidated.OutboundMessages[0]);
        Assert.Contains("\"invalidation_reason\":\"schema-change\"", listInvalidated.OutboundMessages[0]);

        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void QueryReplay001LiveVsReplayParityAtExplicitCheckpoint()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var register = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(register.DispatchSucceeded);

        var addAtCheckpoint = processor.ProcessIncomingText(
            "{\"type\":\"map-set\",\"key\":\"world/d\",\"value\":\"4\"}",
            state
        );
        Assert.True(addAtCheckpoint.DispatchSucceeded);

        var liveBuild = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"latest\"}}",
            state
        );
        Assert.True(liveBuild.DispatchSucceeded);

        var liveRead = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(liveRead.DispatchSucceeded);
        Assert.Contains("\"k\":\"world/d\"", liveRead.OutboundMessages[0]);

        var mutateAfterCheckpoint = processor.ProcessIncomingText(
            "{\"type\":\"map-delete\",\"key\":\"world/d\"}",
            state
        );
        Assert.True(mutateAfterCheckpoint.DispatchSucceeded);

        var replayBuild = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"seq\",\"canonical_seq\":1}}",
            state
        );
        Assert.True(replayBuild.DispatchSucceeded);

        var replayRead = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(replayRead.DispatchSucceeded);
        Assert.Contains("\"k\":\"world/d\"", replayRead.OutboundMessages[0]);

        var liveDigest = ExtractDigest(liveRead.OutboundMessages[0]);
        var replayDigest = ExtractDigest(replayRead.OutboundMessages[0]);
        Assert.Equal(liveDigest, replayDigest);
        Assert.False(string.IsNullOrWhiteSpace(liveDigest));

        Assert.Equal(2, bridge.Commands.Count);
        Assert.Contains("\"MapSet\"", bridge.Commands[0]);
        Assert.Contains("\"MapDelete\"", bridge.Commands[1]);
    }

    [Fact]
    public void QueryReplaySelectorEquivalenceSeqHashAndFrontier()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var register = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(register.DispatchSucceeded);

        var add = processor.ProcessIncomingText(
            "{\"type\":\"map-set\",\"key\":\"world/d\",\"value\":\"4\"}",
            state
        );
        Assert.True(add.DispatchSucceeded);

        var seqBuild = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"seq\",\"canonical_seq\":1}}",
            state
        );
        Assert.True(seqBuild.DispatchSucceeded);
        var canonicalHash = ExtractCheckpointCanonicalHash(seqBuild.OutboundMessages[0]);
        Assert.False(string.IsNullOrWhiteSpace(canonicalHash));

        var seqRead = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(seqRead.DispatchSucceeded);

        var hashBuild = processor.ProcessIncomingText(
            $"{{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{{\"selector\":\"hash\",\"canonical_hash\":\"{canonicalHash}\"}}}}",
            state
        );
        Assert.True(hashBuild.DispatchSucceeded);

        var hashRead = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(hashRead.DispatchSucceeded);

        var frontierBuild = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"frontier\",\"frontier\":[\"seq:1\"]}}",
            state
        );
        Assert.True(frontierBuild.DispatchSucceeded);

        var frontierRead = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(frontierRead.DispatchSucceeded);

        var seqDigest = ExtractDigest(seqRead.OutboundMessages[0]);
        var hashDigest = ExtractDigest(hashRead.OutboundMessages[0]);
        var frontierDigest = ExtractDigest(frontierRead.OutboundMessages[0]);

        Assert.Equal(seqDigest, hashDigest);
        Assert.Equal(seqDigest, frontierDigest);
        Assert.Contains("\"k\":\"world/d\"", seqRead.OutboundMessages[0]);
        Assert.Contains("\"k\":\"world/d\"", hashRead.OutboundMessages[0]);
        Assert.Contains("\"k\":\"world/d\"", frontierRead.OutboundMessages[0]);

        Assert.Single(bridge.Commands);
        Assert.Contains("\"MapSet\"", bridge.Commands[0]);
    }

    [Fact]
    public void QueryCompatReject001SelectorPayloadValidationBoundedTaxonomy()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var register = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(register.DispatchSucceeded);

        var malformedFrontier = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"frontier\",\"frontier\":[\"bad\"]}}",
            state
        );
        var mixedSelectorFields = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"seq\",\"canonical_seq\":1,\"canonical_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}}",
            state
        );
        var invalidHashFormat = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"hash\",\"canonical_hash\":\"1234\"}}",
            state
        );
        var unknownHash = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"hash\",\"canonical_hash\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"}}",
            state
        );

        var reasonClasses = new[]
        {
            ExtractReasonClass(malformedFrontier.OutboundMessages[0]),
            ExtractReasonClass(mixedSelectorFields.OutboundMessages[0]),
            ExtractReasonClass(invalidHashFormat.OutboundMessages[0]),
            ExtractReasonClass(unknownHash.OutboundMessages[0])
        };

        Assert.Equal("reject.checkpoint_selector_invalid", reasonClasses[0]);
        Assert.Equal("reject.checkpoint_selector_invalid", reasonClasses[1]);
        Assert.Equal("reject.checkpoint_selector_invalid", reasonClasses[2]);
        Assert.Equal("reject.checkpoint_not_found", reasonClasses[3]);

        var allowed = new HashSet<string>(StringComparer.Ordinal)
        {
            "reject.checkpoint_selector_invalid",
            "reject.checkpoint_not_found"
        };
        Assert.All(reasonClasses, reasonClass =>
        {
            Assert.NotNull(reasonClass);
            Assert.Contains(reasonClass!, allowed);
        });
    }

    private static string? ExtractDigest(string json)
    {
        const string marker = "\"digest\":\"";
        var start = json.IndexOf(marker, StringComparison.Ordinal);
        if (start < 0)
        {
            return null;
        }

        start += marker.Length;
        var end = json.IndexOf('"', start);
        if (end < 0)
        {
            return null;
        }

        return json[start..end];
    }

    private static string? ExtractCheckpointCanonicalHash(string json)
    {
        const string marker = "\"canonical_hash\":\"";
        var start = json.IndexOf(marker, StringComparison.Ordinal);
        if (start < 0)
        {
            return null;
        }

        start += marker.Length;
        var end = json.IndexOf('"', start);
        if (end < 0)
        {
            return null;
        }

        return json[start..end];
    }

    private static string? ExtractReasonClass(string json)
    {
        const string marker = "\"reason_class\":\"";
        var start = json.IndexOf(marker, StringComparison.Ordinal);
        if (start < 0)
        {
            return null;
        }

        start += marker.Length;
        var end = json.IndexOf('"', start);
        if (end < 0)
        {
            return null;
        }

        return json[start..end];
    }
}
