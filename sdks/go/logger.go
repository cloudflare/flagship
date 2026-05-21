package flagship

import (
	"context"
	"log/slog"
)

// Logger is the minimal logging contract used by the Flagship provider.
type Logger interface {
	DebugContext(context.Context, string, ...any)
	InfoContext(context.Context, string, ...any)
	WarnContext(context.Context, string, ...any)
	ErrorContext(context.Context, string, ...any)
}

func resolveLogger(logger Logger) Logger {
	if logger != nil {
		return logger
	}
	return slog.Default()
}
