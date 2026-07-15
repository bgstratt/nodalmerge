using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;

namespace NodalMerge.DotNetHost.Tests;

public sealed class ProviderCompositionTests
{
    [Fact]
    public void AddNodalMergeHostProviders_RegistersDefaultImplementations()
    {
        var services = new ServiceCollection();

        services.AddNodalMergeHostProviders();

        var serviceProvider = services.BuildServiceProvider();
        var options = serviceProvider.GetRequiredService<NodalMergeHostProviderOptions>();

        Assert.Equal("InMemory", options.NodeStorageProvider);
        Assert.Equal("WsOnly", options.BlobStorageProvider);
        Assert.Equal("Default", options.AuthProvider);

        Assert.NotNull(serviceProvider.GetService<INodeStoreProvider>());
        Assert.NotNull(serviceProvider.GetService<IBlobStoreProvider>());
        Assert.NotNull(serviceProvider.GetService<IBlobUrlResolverProvider>());
        Assert.NotNull(serviceProvider.GetService<IRoomTokenAuthProvider>());
        Assert.NotNull(serviceProvider.GetService<IProviderHealthCheck>());
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForUnknownProvider()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "Unknown"
                }
            )
            .Build();

        var services = new ServiceCollection();

        var ex = Assert.Throws<InvalidOperationException>(
            () => services.AddNodalMergeHostProviders(config)
        );

        Assert.Contains("Unsupported blob storage provider", ex.Message);
    }

    [Fact]
    public void AddNodalMergeHostProviders_SelectsJwtBridgeEmbeddedAuthProvider()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                }
            )
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(config);

        var serviceProvider = services.BuildServiceProvider();
        var auth = serviceProvider.GetRequiredService<IRoomTokenAuthProvider>();

        Assert.Equal("JwtBridgeEmbeddedRoomTokenAuthProvider", auth.GetType().Name);
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForInvalidJwtEmbeddedClockSkew()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:ClockSkewSeconds"] = "999"
                }
            )
            .Build();

        var services = new ServiceCollection();
        var ex = Assert.Throws<InvalidOperationException>(() => services.AddNodalMergeHostProviders(config));
        Assert.Contains("ClockSkewSeconds", ex.Message);
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForInvalidJwtEmbeddedPreviousSigningKey()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:PreviousSigningKeys:0"] = "too-short"
                }
            )
            .Build();

        var services = new ServiceCollection();
        var ex = Assert.Throws<InvalidOperationException>(() => services.AddNodalMergeHostProviders(config));
        Assert.Contains("PreviousSigningKeys", ex.Message);
    }

    [Fact]
    public void AddNodalMergeHostProviders_SelectsSqliteAndFileProviders_WhenConfigured()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-provider-composition", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "Sqlite",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:Sqlite:DbPath"] = dbPath,
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = blobRoot
                }
            )
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(config);
        var serviceProvider = services.BuildServiceProvider();

        var nodeProvider = serviceProvider.GetRequiredService<INodeStoreProvider>();
        var blobProvider = serviceProvider.GetRequiredService<IBlobStoreProvider>();

        Assert.Equal("SqliteNodeStoreProvider", nodeProvider.GetType().Name);
        Assert.Equal("FileBlobStoreProvider", blobProvider.GetType().Name);
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForMissingSqliteDbPath_WhenSqliteIsSelected()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "Sqlite",
                    ["NodalMerge:Storage:Sqlite:DbPath"] = ""
                }
            )
            .Build();

        var services = new ServiceCollection();

        var ex = Assert.Throws<InvalidOperationException>(
            () => services.AddNodalMergeHostProviders(config)
        );

        Assert.Contains("NodalMerge:Storage:Sqlite:DbPath", ex.Message);
    }

    [Fact]
    public void AddNodalMergeHostProviders_SelectsMongoAndS3DelegatedProviders_WhenConfigured()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "Mongo",
                    ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                    ["NodalMerge:Storage:Mongo:ConnectionString"] = "mongodb://localhost:27017",
                    ["NodalMerge:Storage:Mongo:DatabaseName"] = "nodalmerge-test",
                    ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.example"
                }
            )
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(config);

        var nodeDescriptor = services.LastOrDefault(d => d.ServiceType == typeof(INodeStoreProvider));
        var blobResolverDescriptor = services.LastOrDefault(d => d.ServiceType == typeof(IBlobUrlResolverProvider));

        Assert.NotNull(nodeDescriptor);
        Assert.NotNull(blobResolverDescriptor);
        Assert.Equal("MongoNodeStoreProvider", nodeDescriptor!.ImplementationType?.Name);
        Assert.Equal("S3DelegatedBlobUrlResolverProvider", blobResolverDescriptor!.ImplementationType?.Name);
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForMissingS3DelegatedBaseUrl_WhenS3DelegatedIsSelected()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                    ["NodalMerge:Storage:S3Delegated:BaseUrl"] = ""
                }
            )
            .Build();

        var services = new ServiceCollection();

        var ex = Assert.Throws<InvalidOperationException>(() => services.AddNodalMergeHostProviders(config));
        Assert.Contains("NodalMerge:Storage:S3Delegated:BaseUrl", ex.Message);
    }

    [Fact]
    public void AddNodalMergeHostProviders_SelectsChainedRemoteProviders_WhenConfigured()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-provider-composition-chained", Guid.NewGuid().ToString("N"));

        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = tempRoot,
                    ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = "https://blob-origin.example"
                }
            )
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(config);
        var serviceProvider = services.BuildServiceProvider();

        var blobProvider = serviceProvider.GetRequiredService<IBlobStoreProvider>();
        var pushTarget = serviceProvider.GetRequiredService<IRemoteBlobPushTarget>();
        var urlResolver = serviceProvider.GetRequiredService<IBlobUrlResolverProvider>();

        Assert.Equal("ChainedBlobStoreProvider", blobProvider.GetType().Name);
        Assert.Equal("HttpRemoteBlobStoreProvider", pushTarget.GetType().Name);
        Assert.Equal("FileBlobStoreProvider", urlResolver.GetType().Name);
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForMissingRemoteOriginBaseUrl_WhenChainedRemoteIsSelected()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                    ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = ""
                }
            )
            .Build();

        var services = new ServiceCollection();

        var ex = Assert.Throws<InvalidOperationException>(() => services.AddNodalMergeHostProviders(config));
        Assert.Contains("NodalMerge:Storage:RemoteOrigin:BaseUrl", ex.Message);
    }
}
