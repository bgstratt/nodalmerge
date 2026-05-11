namespace ActiveSync.DotNetHost.Runtime;

public static class RuntimeDispatchClosePolicy
{
    public static bool ShouldCloseAfterDispatch(bool closeRequested, bool dispatchSucceeded)
    {
        return closeRequested && dispatchSucceeded;
    }
}
