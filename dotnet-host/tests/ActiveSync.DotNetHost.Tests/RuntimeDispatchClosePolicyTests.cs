using ActiveSync.DotNetHost.Runtime;

namespace ActiveSync.DotNetHost.Tests;

public class RuntimeDispatchClosePolicyTests
{
    [Fact]
    public void Close_requested_and_dispatch_successful_returns_true()
    {
        var shouldClose = RuntimeDispatchClosePolicy.ShouldCloseAfterDispatch(
            closeRequested: true,
            dispatchSucceeded: true
        );

        Assert.True(shouldClose);
    }

    [Fact]
    public void Close_requested_but_dispatch_failed_returns_false()
    {
        var shouldClose = RuntimeDispatchClosePolicy.ShouldCloseAfterDispatch(
            closeRequested: true,
            dispatchSucceeded: false
        );

        Assert.False(shouldClose);
    }

    [Fact]
    public void Close_not_requested_returns_false_even_when_dispatch_successful()
    {
        var shouldClose = RuntimeDispatchClosePolicy.ShouldCloseAfterDispatch(
            closeRequested: false,
            dispatchSucceeded: true
        );

        Assert.False(shouldClose);
    }
}
