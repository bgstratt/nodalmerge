using System.Diagnostics;
using Microsoft.Extensions.Configuration;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 4.3 (blob-cas-remediation.md): <c>PutPath</c>/<c>GetPath</c> were
/// removed from <see cref="S3DelegatedBlobOptions"/> by the presign-protocol-v1
/// migration (see that record's doc comment), but the options binder
/// (<see cref="S3DelegatedBlobOptions.FromConfiguration"/>) silently drops
/// unknown keys — so a deployment whose config still sets them got no signal
/// beyond <c>PresignPath</c> quietly defaulting to <c>/v1/blobs/presign</c>,
/// degrading to the WS blob fallback. Pre-fix, <c>WarnOnStaleKeys</c> did not
/// exist at all: nothing warned, loudly or otherwise.
///
/// Per plan principle 3 this is a WARN, never a hard-fail (don't brick a
/// running upgrade over stale-but-harmless config) — so there is no
/// exception to assert on. The only seam this host exposes for a
/// composition-time diagnostic is <see cref="Trace"/> (this codebase has no
/// logging abstraction outside the built <c>WebApplication</c>'s DI
/// container — see <c>CapabilityProfileExpander.ClampToCeiling</c>'s
/// identical precedent and its test,
/// <c>CapabilityProfileLimitsClampTests.Clamp_warns_with_field_requested_value_and_ceiling</c>,
/// which this file's <see cref="CapturingTraceListener"/> is copied from). No
/// sibling startup-warning test asserts via any other seam.
/// </summary>
public sealed class S3DelegatedBlobOptionsStaleKeyGuardTests
{
    [Fact]
    public void Warns_naming_both_removed_keys_when_both_are_set()
    {
        var configuration = ConfigWith(new Dictionary<string, string?>
        {
            ["NodalMerge:Storage:S3Delegated:PutPath"] = "/legacy/put",
            ["NodalMerge:Storage:S3Delegated:GetPath"] = "/legacy/get"
        });

        var messages = CaptureTraceWarnings(() => S3DelegatedBlobOptions.WarnOnStaleKeys(configuration));

        // Trace.TraceWarning emits a "testhost Warning: 0 : " header entry in
        // addition to the message itself (same shape the capability-profile
        // clamp tests filter around) — select on content, don't assert the
        // whole collection's count.
        var warning = Assert.Single(messages, m => m.Contains("NodalMerge:Storage:S3Delegated"));
        Assert.Contains("PutPath", warning);
        Assert.Contains("GetPath", warning);
        Assert.Contains("PresignPath", warning);
    }

    [Fact]
    public void Warns_naming_only_the_one_stale_key_present()
    {
        var configuration = ConfigWith(new Dictionary<string, string?>
        {
            ["NodalMerge:Storage:S3Delegated:PutPath"] = "/legacy/put"
        });

        var messages = CaptureTraceWarnings(() => S3DelegatedBlobOptions.WarnOnStaleKeys(configuration));

        var warning = Assert.Single(messages, m => m.Contains("NodalMerge:Storage:S3Delegated"));
        Assert.Contains("PutPath", warning);
        Assert.DoesNotContain("GetPath", warning);
    }

    [Fact]
    public void Does_not_warn_when_neither_stale_key_is_set()
    {
        var configuration = ConfigWith(new Dictionary<string, string?>
        {
            ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://origin.test",
            ["NodalMerge:Storage:S3Delegated:PresignPath"] = "/v1/blobs/presign"
        });

        var messages = CaptureTraceWarnings(() => S3DelegatedBlobOptions.WarnOnStaleKeys(configuration));

        // Filter on content, don't assert the whole collection is empty:
        // Trace.Listeners is process-global, so a test running in a parallel
        // xunit collection (e.g. CapabilityProfileLimitsClampTests, which
        // warns via the same Trace channel) can add unrelated entries to
        // THIS listener's capture during the same window. Same reasoning as
        // that file's own sentinel-filtering comment.
        Assert.DoesNotContain(messages, m => m.Contains("NodalMerge:Storage:S3Delegated"));
    }

    [Fact]
    public void Does_not_warn_when_stale_keys_are_present_but_empty()
    {
        var configuration = ConfigWith(new Dictionary<string, string?>
        {
            ["NodalMerge:Storage:S3Delegated:PutPath"] = "",
            ["NodalMerge:Storage:S3Delegated:GetPath"] = ""
        });

        var messages = CaptureTraceWarnings(() => S3DelegatedBlobOptions.WarnOnStaleKeys(configuration));

        Assert.DoesNotContain(messages, m => m.Contains("NodalMerge:Storage:S3Delegated"));
    }

    [Fact]
    public void Does_not_throw_when_configuration_is_null()
    {
        var exception = Record.Exception(() => S3DelegatedBlobOptions.WarnOnStaleKeys(null));

        Assert.Null(exception);
    }

    private static IConfiguration ConfigWith(Dictionary<string, string?> entries)
    {
        return new ConfigurationBuilder().AddInMemoryCollection(entries).Build();
    }

    /// <summary>
    /// <see cref="Trace.Listeners"/> is a process-global collection, so this
    /// capture is NOT isolated from other tests' Trace output running in a
    /// parallel xunit collection during the same window (verified: this
    /// flaked once against `CapabilityProfileLimitsClampTests`' clamp
    /// warnings before the "does not warn" assertions below were switched
    /// from `Assert.Empty` to a content-filtered `Assert.DoesNotContain` —
    /// same reasoning as that file's own sentinel-filtering comment). Every
    /// assertion against this capture must filter on
    /// "NodalMerge:Storage:S3Delegated" content, never assert the whole
    /// collection's shape.
    /// </summary>
    private static IReadOnlyList<string> CaptureTraceWarnings(Action action)
    {
        var listener = new CapturingTraceListener();
        Trace.Listeners.Add(listener);
        try
        {
            action();
        }
        finally
        {
            Trace.Listeners.Remove(listener);
        }
        return listener.Messages;
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
