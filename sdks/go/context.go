package flagship

import (
	"net/url"
	"reflect"
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
	case time.Time:
		return v.Format(time.RFC3339Nano), true
	case int, int8, int16, int32, int64:
		return strconv.FormatInt(reflect.ValueOf(v).Int(), 10), true
	case uint, uint8, uint16, uint32, uint64:
		return strconv.FormatUint(reflect.ValueOf(v).Uint(), 10), true
	case float32, float64:
		value := reflect.ValueOf(v)
		return strconv.FormatFloat(value.Float(), 'f', -1, int(value.Type().Bits())), true
	default:
		return "", false
	}
}
