package flagship

import (
	"fmt"
	"math"
	"net/url"
	"reflect"
	"strconv"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

const maxContextDepth = 100

type normalizedContext struct {
	values       map[string]any
	requiresPost bool
}

func normalizeContext(flatCtx openfeature.FlattenedContext) (normalizedContext, error) {
	values := make(map[string]any, len(flatCtx))
	requiresPost := false
	for key, value := range flatCtx {
		normalized, structured, err := normalizeContextValue(value, key, 0)
		if err != nil {
			return normalizedContext{}, err
		}
		values[key] = normalized
		requiresPost = requiresPost || structured
	}
	return normalizedContext{values: values, requiresPost: requiresPost}, nil
}

func contextToQueryParams(flatCtx openfeature.FlattenedContext) (url.Values, error) {
	normalized, err := normalizeContext(flatCtx)
	if err != nil {
		return nil, err
	}
	if normalized.requiresPost {
		return nil, newError(ErrorCodeInvalidContext, "structured evaluation context requires a JSON request body", 0, nil)
	}

	params := url.Values{}
	for key, value := range normalized.values {
		serialized, ok := serializeContextValue(value)
		if !ok {
			return nil, invalidContextError(key, value)
		}
		params.Set(key, serialized)
	}
	return params, nil
}

func normalizeContextValue(value any, path string, depth int) (any, bool, error) {
	if depth > maxContextDepth {
		return nil, false, newError(ErrorCodeInvalidContext, fmt.Sprintf("context attribute %q exceeds maximum nesting depth", path), 0, nil)
	}
	if value == nil {
		return nil, true, nil
	}

	switch v := value.(type) {
	case string, bool:
		return v, false, nil
	case time.Time:
		return v.Format(time.RFC3339Nano), false, nil
	case float32:
		if math.IsNaN(float64(v)) || math.IsInf(float64(v), 0) {
			return nil, false, invalidContextError(path, value)
		}
		return v, false, nil
	case float64:
		if math.IsNaN(v) || math.IsInf(v, 0) {
			return nil, false, invalidContextError(path, value)
		}
		return v, false, nil
	case int, int8, int16, int32, int64, uint, uint8, uint16, uint32, uint64:
		return v, false, nil
	}

	reflected := reflect.ValueOf(value)
	if (reflected.Kind() == reflect.Map || reflected.Kind() == reflect.Slice) && reflected.IsNil() {
		return nil, true, nil
	}
	switch reflected.Kind() {
	case reflect.Map:
		if reflected.Type().Key().Kind() != reflect.String {
			return nil, false, invalidContextError(path, value)
		}
		result := make(map[string]any, reflected.Len())
		iterator := reflected.MapRange()
		for iterator.Next() {
			key := iterator.Key().String()
			normalized, _, err := normalizeContextValue(iterator.Value().Interface(), path+"."+key, depth+1)
			if err != nil {
				return nil, false, err
			}
			result[key] = normalized
		}
		return result, true, nil
	case reflect.Slice, reflect.Array:
		result := make([]any, reflected.Len())
		for index := 0; index < reflected.Len(); index++ {
			normalized, _, err := normalizeContextValue(reflected.Index(index).Interface(), fmt.Sprintf("%s[%d]", path, index), depth+1)
			if err != nil {
				return nil, false, err
			}
			result[index] = normalized
		}
		return result, true, nil
	default:
		return nil, false, invalidContextError(path, value)
	}
}

func serializeContextValue(value any) (string, bool) {
	switch v := value.(type) {
	case string:
		return v, true
	case bool:
		return strconv.FormatBool(v), true
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
