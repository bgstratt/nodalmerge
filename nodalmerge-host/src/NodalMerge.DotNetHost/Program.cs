using NodalMerge.DotNetHost;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Composition;

var builder = WebApplication.CreateBuilder(args);
builder.Services.AddNodalMergeHostProviders(builder.Configuration);
builder.Services.AddNodalMergeRuntimeCore(builder.Configuration);
var app = builder.Build();
app.MapNodalMergeEndpoints();
app.Run();
