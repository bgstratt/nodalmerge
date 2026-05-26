using System.Reflection;
using System.Runtime.InteropServices;
using System.Collections.Generic;

namespace NodalMerge.DotNetHost.Ffi;

public static class NativeLibraryResolver
{
    private static bool _configured;

    public static void Configure()
    {
        if (_configured)
        {
            return;
        }

        NativeLibrary.SetDllImportResolver(
            typeof(NativeMethods).Assembly,
            ResolveLibrary
        );

        _configured = true;
    }

    private static nint ResolveLibrary(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (!string.Equals(libraryName, NativeMethods.LibraryName, StringComparison.Ordinal))
        {
            return nint.Zero;
        }

        var explicitPath = Environment.GetEnvironmentVariable("NODALMERGE_HOST_FFI_DLL");
        if (string.IsNullOrWhiteSpace(explicitPath))
        {
            explicitPath = Environment.GetEnvironmentVariable("ACTIVESYNC_HOST_FFI_DLL");
        }
        if (!string.IsNullOrWhiteSpace(explicitPath) && File.Exists(explicitPath))
        {
            return NativeLibrary.Load(explicitPath);
        }

        foreach (var candidate in BuildCandidatePaths())
        {
            if (!File.Exists(candidate))
            {
                continue;
            }

            try
            {
                return NativeLibrary.Load(candidate);
            }
            catch
            {
                // Keep scanning candidates; a bad candidate should not block
                // discovery of a valid native library path.
            }
        }

        try
        {
            return NativeLibrary.Load(libraryName, assembly, searchPath);
        }
        catch
        {
            return nint.Zero;
        }

    }

    private static IEnumerable<string> BuildCandidatePaths()
    {
        var fileName = GetPlatformLibraryFileName();
        var baseDir = AppContext.BaseDirectory;
        var cwd = Directory.GetCurrentDirectory();

        yield return Path.Combine(baseDir, fileName);
        yield return Path.Combine(cwd, fileName);

        // Common workspace cargo target locations when running from repo root.
        yield return Path.Combine(cwd, "target", "debug", fileName);
        yield return Path.Combine(cwd, "target", "release", fileName);

        // Fallback for standalone crate target layout.
        yield return Path.Combine(cwd, "host-ffi", "target", "debug", fileName);
        yield return Path.Combine(cwd, "host-ffi", "target", "release", fileName);

        // Common relative paths when running from dotnet-host/src/*.
        yield return Path.Combine(cwd, "..", "..", "target", "debug", fileName);
        yield return Path.Combine(cwd, "..", "..", "target", "release", fileName);
        yield return Path.Combine(cwd, "..", "..", "..", "target", "debug", fileName);
        yield return Path.Combine(cwd, "..", "..", "..", "target", "release", fileName);
        yield return Path.Combine(cwd, "..", "..", "..", "..", "target", "debug", fileName);
        yield return Path.Combine(cwd, "..", "..", "..", "..", "target", "release", fileName);

        // Sibling workspace fallback for multi-repo local dev.
        foreach (var siblingRepoName in new[] { "activeSync", "activesync" })
        {
            yield return Path.Combine(cwd, "..", siblingRepoName, "target", "debug", fileName);
            yield return Path.Combine(cwd, "..", siblingRepoName, "target", "release", fileName);
            yield return Path.Combine(cwd, "..", "..", siblingRepoName, "target", "debug", fileName);
            yield return Path.Combine(cwd, "..", "..", siblingRepoName, "target", "release", fileName);
            yield return Path.Combine(cwd, "..", "..", "..", siblingRepoName, "target", "debug", fileName);
            yield return Path.Combine(cwd, "..", "..", "..", siblingRepoName, "target", "release", fileName);
            yield return Path.Combine(cwd, "..", "..", "..", "..", siblingRepoName, "target", "debug", fileName);
            yield return Path.Combine(cwd, "..", "..", "..", "..", siblingRepoName, "target", "release", fileName);
        }
    }

    private static string GetPlatformLibraryFileName()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            return "nodalmerge_host_ffi.dll";
        }

        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
        {
            return "libnodalmerge_host_ffi.dylib";
        }

        return "libnodalmerge_host_ffi.so";
    }
}
