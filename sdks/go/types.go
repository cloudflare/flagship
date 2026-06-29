package flagship

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

// DefaultBaseURL is the default Cloudflare API base URL used when Options.AppID
// and Options.AccountID are provided.
const DefaultBaseURL = "https://api.cloudflare.com"

const (
	defaultTimeout    = 5 * time.Second
	defaultRetries    = 1
	defaultRetryDelay = time.Second
	defaultCacheSize  = 1000
	maxRetries        = 10
	maxRetryDelay     = 30 * time.Second
)

// HeaderFactory returns request headers for a single Flagship API request.
// It is invoked once per attempt, so callers can use it for rotating tokens.
type HeaderFactory func(context.Context) (http.Header, error)

// Options configures a Flagship HTTP client and OpenFeature provider.
//
// Provide either AppID plus AccountID, or Endpoint. Endpoint must be an
// absolute URL. Headers and HeadersFactory override AuthToken for the
// Authorization header.
type Options struct {
	AppID     string
	AccountID string
	Endpoint  string
	BaseURL   string

	AuthToken      string
	Headers        http.Header
	HeadersFactory HeaderFactory

	HTTPClient *http.Client
	Timeout    time.Duration
	Retries    int
	// DisableRetries disables transient-error retries. Retries is otherwise
	// defaulted to 1 when left as the Go zero value.
	DisableRetries bool
	RetryDelay     time.Duration

	Logging bool
	Logger  Logger
	Hooks   []openfeature.Hook

	// CacheTTL enables an in-memory TTL + LRU response cache when greater than 0.
	// Cached values may be up to this duration stale.
	CacheTTL time.Duration
	// CacheMaxSize limits cached entries. Defaults to 1000 when CacheTTL is set.
	CacheMaxSize int
}

// EvaluationReason is the reason returned by the Flagship evaluation API.
type EvaluationReason string

const (
	ReasonTargetingMatch EvaluationReason = "TARGETING_MATCH"
	ReasonDefault        EvaluationReason = "DEFAULT"
	ReasonDisabled       EvaluationReason = "DISABLED"
	ReasonSplit          EvaluationReason = "SPLIT"
)

// EvaluationResponse is a successful response from the Flagship evaluation API.
type EvaluationResponse struct {
	FlagKey string
	Value   any
	Variant string
	Reason  EvaluationReason
}

// ErrorCode identifies an error produced by the Flagship HTTP client.
type ErrorCode string

const (
	ErrorCodeFlagNotFound   ErrorCode = "FLAG_NOT_FOUND"
	ErrorCodeBadRequest     ErrorCode = "BAD_REQUEST"
	ErrorCodeNetwork        ErrorCode = "NETWORK_ERROR"
	ErrorCodeTimeout        ErrorCode = "TIMEOUT_ERROR"
	ErrorCodeParse          ErrorCode = "PARSE_ERROR"
	ErrorCodeInvalidContext ErrorCode = "INVALID_CONTEXT"
	ErrorCodeGeneral        ErrorCode = "GENERAL"
)

// Error is returned by FlagshipClient for abnormal HTTP, network, parse, or
// context serialization conditions.
type Error struct {
	Code       ErrorCode
	Message    string
	StatusCode int
	Err        error
}

func (e *Error) Error() string {
	if e == nil {
		return ""
	}
	if e.Message != "" {
		return e.Message
	}
	return string(e.Code)
}

func (e *Error) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Err
}

func newError(code ErrorCode, message string, statusCode int, err error) *Error {
	return &Error{Code: code, Message: message, StatusCode: statusCode, Err: err}
}

func asFlagshipError(err error) (*Error, bool) {
	var flagshipErr *Error
	if errors.As(err, &flagshipErr) {
		return flagshipErr, true
	}
	return nil, false
}

func invalidContextError(key string, value any) *Error {
	return newError(
		ErrorCodeInvalidContext,
		fmt.Sprintf("context attribute %q has unsupported type %T; use string, number, bool, or time.Time", key, value),
		0,
		nil,
	)
}
