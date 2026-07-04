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

    internal static nint ResolveLibrary(string libraryName, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (string.Equals(libraryName, NativeMethods.LibraryName, StringComparison.Ordinal))
        {
            return LoadFromCandidates(
                Environment.GetEnvironmentVariable("NODALMERGE_HOST_FFI_DLL"),
                GetHostPlatformLibraryFileName(),
                assembly,
                searchPath
            );
        }

        if (string.Equals(libraryName, LocalNativeMethods.LibraryName, StringComparison.Ordinal))
        {
            return LoadFromCandidates(
                Environment.GetEnvironmentVariable("NODALMERGE_LOCAL_FFI_DLL"),
                GetLocalPlatformLibraryFileName(),
                assembly,
                searchPath
            );
        }

        return nint.Zero;
    }

    private static nint LoadFromCandidates(
        string? explicitPath,
        string fileName,
        Assembly assembly,
        DllImportSearchPath? searchPath
    )
    {
        if (!string.IsNullOrWhiteSpace(explicitPath) && File.Exists(explicitPath))
        {
            return NativeLibrary.Load(explicitPath);
        }

        foreach (var candidate in BuildCandidatePaths(fileName))
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
            return NativeLibrary.Load(fileName, assembly, searchPath);
        }
        catch
        {
            return nint.Zero;
        }
    }

    private static IEnumerable<string> BuildCandidatePaths(string fileName)
    {
        var baseDir = AppContext.BaseDirectory;
        var cwd = Directory.GetCurrentDirectory();

        yield return Path.Combine(baseDir, fileName);
        yield return Path.Combine(cwd, fileName);

        // NuGet-deployed native assets. Probed before the cargo target
        // fallbacks below so a host consuming the packaged runtime never
        // silently picks up a stale dev build from a sibling repo checkout.
        // (NODALMERGE_HOST_FFI_DLL / NODALMERGE_LOCAL_FFI_DLL still override
        // everything for deliberate local-dev pinning.)
        var rid = RuntimeInformation.RuntimeIdentifier;
        yield return Path.Combine(baseDir, "runtimes", rid, "native", fileName);
        if (!string.IsNullOrEmpty(rid))
        {
            var dashIndex = rid.LastIndexOf('-');
            if (dashIndex > 0)
            {
                // e.g. "win10-x64" -> "win-x64" style fallback for portable RIDs.
                var arch = rid[(dashIndex + 1)..];
                var portableRid = RuntimeInformation.IsOSPlatform(OSPlatform.Windows)
                    ? $"win-{arch}"
                    : RuntimeInformation.IsOSPlatform(OSPlatform.OSX)
                        ? $"osx-{arch}"
                        : $"linux-{arch}";
                if (!string.Equals(portableRid, rid, StringComparison.Ordinal))
                {
                    yield return Path.Combine(baseDir, "runtimes", portableRid, "native", fileName);
                }
            }
        }

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
        foreach (var siblingRepoName in new[] { "nodalmerge" })
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

        // Bounded upward walk from the assembly location and cwd looking for a
        // cargo target directory. Covers runners whose base/cwd is a deep bin
        // output (e.g. `dotnet test` runs from tests/<proj>/bin/<cfg>/<tfm>,
        // six levels below the repo root's target/), which the fixed-depth
        // chains above don't reach.
        foreach (var start in new[] { baseDir, cwd })
        {
            var dir = string.IsNullOrEmpty(start) ? null : Path.GetFullPath(start);
            for (var depth = 0; depth < 8 && !string.IsNullOrEmpty(dir); depth++)
            {
                yield return Path.Combine(dir, "target", "debug", fileName);
                yield return Path.Combine(dir, "target", "release", fileName);
                dir = Path.GetDirectoryName(dir.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
            }
        }
    }

    private static string GetHostPlatformLibraryFileName()
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

    private static string GetLocalPlatformLibraryFileName()
    {
        if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows))
        {
            return "nodalmerge_runtime_local_ffi.dll";
        }

        if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX))
        {
            return "libnodalmerge_runtime_local_ffi.dylib";
        }

        return "libnodalmerge_runtime_local_ffi.so";
    }
}
