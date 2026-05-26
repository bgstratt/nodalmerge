using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

internal static class ConfigurationKeyFallback
{
    public static IConfigurationSection? GetSection(
        IConfiguration? configuration,
        string primarySectionName,
        string legacySectionName)
    {
        var primary = configuration?.GetSection(primarySectionName);
        if (primary is not null && primary.Exists())
        {
            return primary;
        }

        var legacy = configuration?.GetSection(legacySectionName);
        if (legacy is not null && legacy.Exists())
        {
            return legacy;
        }

        return primary;
    }
}