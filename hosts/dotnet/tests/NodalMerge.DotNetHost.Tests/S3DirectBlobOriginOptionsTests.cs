using Microsoft.Extensions.Configuration;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Pins <see cref="S3DirectBlobOriginOptions.FromConfiguration"/> parsing —
/// most importantly the <c>Compression</c> default, which slice 3.3 of
/// nodalmerge-studio/plans/blob-cas-remediation.md (finding #6) flipped from
/// <c>Zstd</c> to <c>Off</c>: a default-on client-side zstd upload stores a
/// zstd frame in the bucket that the web SDK's presigned-GET reader could not
/// decode on Safari/Node (they don't transparently decode
/// <c>Content-Encoding: zstd</c> the way Chrome 123+/FF 126+ do), so
/// correctness depended on which browser happened to fetch. Compression is
/// now explicitly opt-in.
/// </summary>
public sealed class S3DirectBlobOriginOptionsTests
{
    private static IConfiguration BuildConfiguration(Dictionary<string, string?> values)
    {
        return new ConfigurationBuilder().AddInMemoryCollection(values).Build();
    }

    [Fact]
    public void Compression_defaults_to_Off_when_unconfigured()
    {
        var options = S3DirectBlobOriginOptions.FromConfiguration(BuildConfiguration([]));

        Assert.Equal("Off", options.Compression);
    }

    [Fact]
    public void Compression_defaults_to_Off_when_blank()
    {
        var config = BuildConfiguration(new Dictionary<string, string?>
        {
            [$"{S3DirectBlobOriginOptions.SectionName}:Compression"] = "   "
        });

        Assert.Equal("Off", S3DirectBlobOriginOptions.FromConfiguration(config).Compression);
    }

    [Fact]
    public void Compression_Zstd_remains_available_as_explicit_opt_in()
    {
        var config = BuildConfiguration(new Dictionary<string, string?>
        {
            [$"{S3DirectBlobOriginOptions.SectionName}:Compression"] = "Zstd"
        });

        var options = S3DirectBlobOriginOptions.FromConfiguration(config);

        Assert.Equal("Zstd", options.Compression);
        // Opt-in still rides the pre-flip tuning defaults.
        Assert.Equal(3, options.CompressionLevel);
        Assert.Equal(4096, options.CompressionMinBytes);
    }

    [Fact]
    public void Compression_unknown_mode_throws_at_parse_time()
    {
        var config = BuildConfiguration(new Dictionary<string, string?>
        {
            [$"{S3DirectBlobOriginOptions.SectionName}:Compression"] = "Gzip"
        });

        var ex = Assert.Throws<InvalidOperationException>(
            () => S3DirectBlobOriginOptions.FromConfiguration(config)
        );
        Assert.Contains("Compression", ex.Message);
    }
}
