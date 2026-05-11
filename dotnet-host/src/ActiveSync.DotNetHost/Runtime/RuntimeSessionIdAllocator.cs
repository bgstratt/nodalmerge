using System.Threading;

namespace ActiveSync.DotNetHost.Runtime;

public sealed class RuntimeSessionIdAllocator
{
    private long _nextSessionId;

    public RuntimeSessionIdAllocator(long seed = 1)
    {
        _nextSessionId = seed - 1;
    }

    public ulong Next()
    {
        var id = Interlocked.Increment(ref _nextSessionId);
        return (ulong)id;
    }
}
