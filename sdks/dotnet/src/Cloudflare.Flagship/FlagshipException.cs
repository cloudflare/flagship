using System.Net;

namespace Cloudflare.Flagship;

public enum FlagshipErrorCode
{
    FlagNotFound,
    BadRequest,
    InvalidContext,
    NetworkError,
    TimeoutError,
    ParseError,
    General
}

/// <summary>An evaluation failure with a stable code and optional HTTP status.</summary>
public sealed class FlagshipException : Exception
{
    public FlagshipErrorCode Code { get; }
    public HttpStatusCode? StatusCode { get; }

    public FlagshipException(FlagshipErrorCode code, string message,
        HttpStatusCode? statusCode = null, Exception? innerException = null)
        : base(message, innerException)
    {
        Code = code;
        StatusCode = statusCode;
    }
}
