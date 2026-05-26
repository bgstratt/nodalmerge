namespace NodalMerge.Host.Composition;

public sealed class SidecarAuthProviderException : Exception
{
    public SidecarAuthProviderException(string message, bool isTimeout, int? statusCode = null, Exception? innerException = null)
        : base(message, innerException)
    {
        IsTimeout = isTimeout;
        StatusCode = statusCode;
    }

    public bool IsTimeout { get; }

    public int? StatusCode { get; }
}
