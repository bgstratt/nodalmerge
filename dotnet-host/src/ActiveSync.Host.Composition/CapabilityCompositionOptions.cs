using Microsoft.Extensions.Configuration;

namespace ActiveSync.Host.Composition;

public sealed record CapabilityCompositionOptions(
    bool Enabled,
    string? ProfilePath
)
{
    public const string SectionName = "ActiveSync:Auth:CapabilityComposition";

    public static CapabilityCompositionOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        var enabled = bool.TryParse(section?["Enabled"], out var parsedEnabled) && parsedEnabled;

        return new CapabilityCompositionOptions(
            Enabled: enabled,
            ProfilePath: section?["ProfilePath"]
        );
    }

    public void Validate()
    {
        if (!Enabled)
        {
            return;
        }

        if (string.IsNullOrWhiteSpace(ProfilePath))
        {
            throw new InvalidOperationException($"{SectionName}:ProfilePath is required when Enabled=true");
        }

        if (!File.Exists(ProfilePath))
        {
            throw new InvalidOperationException($"{SectionName}:ProfilePath does not exist: {ProfilePath}");
        }
    }
}
