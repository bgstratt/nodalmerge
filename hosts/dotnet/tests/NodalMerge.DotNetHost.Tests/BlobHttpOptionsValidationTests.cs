namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 4.3 (blob-cas-remediation.md): <see cref="BlobHttpOptions.Validate"/>
/// must reject a non-positive <see cref="BlobHttpOptions.MaxBlobBytes"/> at
/// startup. Pre-fix, <see cref="BlobHttpOptions"/> had no <c>Validate()</c>
/// method at all: a negative value threw an unhandled
/// <see cref="ArgumentOutOfRangeException"/> from the first chunked PUT's
/// buffer allocation (surfacing as a 500, not a config error) and a zero
/// value silently 413'd every PUT forever with no diagnostic. This file pins
/// the fail-fast replacement.
/// </summary>
public sealed class BlobHttpOptionsValidationTests
{
    [Theory]
    [InlineData(0)]
    [InlineData(-1)]
    [InlineData(-1024)]
    public void Validate_rejects_non_positive_MaxBlobBytes(long maxBlobBytes)
    {
        var options = new BlobHttpOptions(AuthToken: null, MaxBlobBytes: maxBlobBytes);

        var ex = Assert.Throws<InvalidOperationException>(options.Validate);
        Assert.Contains("MaxBlobBytes", ex.Message);
        Assert.Contains(BlobHttpOptions.SectionName, ex.Message);
    }

    [Theory]
    [InlineData(1)]
    [InlineData(BlobHttpOptions.DefaultMaxBlobBytes)]
    [InlineData(long.MaxValue)]
    public void Validate_accepts_positive_MaxBlobBytes(long maxBlobBytes)
    {
        var options = new BlobHttpOptions(AuthToken: null, MaxBlobBytes: maxBlobBytes);

        var exception = Record.Exception(options.Validate);

        Assert.Null(exception);
    }
}
