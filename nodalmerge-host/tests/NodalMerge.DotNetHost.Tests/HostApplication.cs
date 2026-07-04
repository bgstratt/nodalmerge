using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost;

/// <summary>
/// Test-side successor of the deleted <c>HostApplication.Build()</c> (removed when
/// NodalMerge.DotNetHost became an embeddable library, commit 3a4c16f3). Composes a
/// WebApplication exactly the way library consumers do — <c>AddNodalMergeHostProviders</c>
/// + <c>AddNodalMergeRuntimeCore</c> + <c>MapNodalMergeEndpoints</c> — while keeping the
/// hook points (services / web host / configuration) the integration tests rely on.
/// </summary>
public static class HostApplication
{
    public static WebApplication Build(
        string[] args,
        Action<IServiceCollection>? configureServices = null,
        Action<IWebHostBuilder>? configureWebHost = null,
        Action<ConfigurationManager>? configureConfiguration = null
    )
    {
        var builder = WebApplication.CreateBuilder(args);
        configureWebHost?.Invoke(builder.WebHost);
        configureConfiguration?.Invoke(builder.Configuration);

        builder.Services.AddNodalMergeHostProviders(builder.Configuration);
        builder.Services.AddNodalMergeRuntimeCore(builder.Configuration);

        configureServices?.Invoke(builder.Services);

        var app = builder.Build();
        app.MapNodalMergeEndpoints();
        return app;
    }
}
