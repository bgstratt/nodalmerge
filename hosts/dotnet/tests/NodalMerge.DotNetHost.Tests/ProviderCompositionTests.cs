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

    [Fact]
    public void AddNodalMergeHostProviders_ChainedRemote_DefaultsS3DirectDisabled_AndPreservesTwoLinkChain()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-provider-composition-chained-s3off", Guid.NewGuid().ToString("N"));

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

        // Slice 4.3: S3Direct:Enabled defaults to false, which must
        // reproduce the pre-4.3 two-link chain exactly — same registrations
        // as AddNodalMergeHostProviders_SelectsChainedRemoteProviders_WhenConfigured,
        // and no S3DirectBlobStoreProvider/RemoteBlobLinkAggregator
        // registered at all.
        var blobProvider = serviceProvider.GetRequiredService<IBlobStoreProvider>();
        Assert.Equal("ChainedBlobStoreProvider", blobProvider.GetType().Name);
        Assert.Null(services.LastOrDefault(d => d.ServiceType == typeof(S3DirectBlobStoreProvider)));
        Assert.Null(services.LastOrDefault(d => d.ServiceType == typeof(RemoteBlobLinkAggregator)));
    }

    [Fact]
    public void AddNodalMergeHostProviders_ChainedRemote_S3DirectEnabled_GrowsChainToThreeLinks()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-provider-composition-chained-s3on", Guid.NewGuid().ToString("N"));

        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = tempRoot,
                    ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = "https://blob-origin.example",
                    ["NodalMerge:Storage:S3Direct:Enabled"] = "true"
                }
            )
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(config);
        var serviceProvider = services.BuildServiceProvider();

        // The composed IBlobStoreProvider is still ChainedBlobStoreProvider
        // — the one and only verify gate — regardless of how many remote
        // links are configured underneath it.
        var blobProvider = serviceProvider.GetRequiredService<IBlobStoreProvider>();
        Assert.Equal("ChainedBlobStoreProvider", blobProvider.GetType().Name);

        Assert.NotNull(serviceProvider.GetService<S3DirectBlobStoreProvider>());
        var aggregator = serviceProvider.GetService<RemoteBlobLinkAggregator>();
        Assert.NotNull(aggregator);

        // The reconcile-sweep push target is unchanged: still the relay
        // link only (see ServiceCollectionExtensions' comment on why).
        var pushTarget = serviceProvider.GetRequiredService<IRemoteBlobPushTarget>();
        Assert.Equal("HttpRemoteBlobStoreProvider", pushTarget.GetType().Name);
    }

    [Fact]
    public void AddNodalMergeHostProviders_ThrowsForInvalidS3DirectCompression_WhenChainedRemoteAndS3DirectEnabled()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                    ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = "https://blob-origin.example",
                    ["NodalMerge:Storage:S3Direct:Enabled"] = "true",
                    ["NodalMerge:Storage:S3Direct:Compression"] = "Gzip"
                }
            )
            .Build();

        var services = new ServiceCollection();

        var ex = Assert.Throws<InvalidOperationException>(() => services.AddNodalMergeHostProviders(config));
        Assert.Contains("NodalMerge:Storage:S3Direct:Compression", ex.Message);
    }
}
