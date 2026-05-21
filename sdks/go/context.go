package flagship

import (
	"net/url"
	"strconv"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func contextToQueryParams(flatCtx openfeature.FlattenedContext) (url.Values, error) {
	params := url.Values{}
	for key, value := range flatCtx {
		if value == nil {
			continue
		}

		serialized, ok := serializeContextValue(value)
		if !ok {
			return nil, invalidContextError(key, value)
		}
		params.Set(key, serialized)
	}
	return params, nil
}

func serializeContextValue(value any) (string, bool) {
	switch v := value.(type) {
	case string:
		return v, true
	case bool:
		return strconv.FormatBool(v), true
	case int:
		return strconv.FormatInt(int64(v), 10), true
	case int8:
		return strconv.FormatInt(int64(v), 10), true
	case int16:
		return strconv.FormatInt(int64(v), 10), true
	case int32:
		return strconv.FormatInt(int64(v), 10), true
	case int64:
		return strconv.FormatInt(v, 10), true
	case uint:
		return strconv.FormatUint(uint64(v), 10), true
	case uint8:
		return strconv.FormatUint(uint64(v), 10), true
	case uint16:
		return strconv.FormatUint(uint64(v), 10), true
	case uint32:
		return strconv.FormatUint(uint64(v), 10), true
	case uint64:
		return strconv.FormatUint(v, 10), true
	case float32:
		return strconv.FormatFloat(float64(v), 'f', -1, 32), true
	case float64:
		return strconv.FormatFloat(v, 'f', -1, 64), true
	case time.Time:
		return v.Format(time.RFC3339Nano), true
	default:
		return "", false
	}
}
