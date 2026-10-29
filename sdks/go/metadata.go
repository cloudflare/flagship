package flagship

import (
	"encoding/json"

	"github.com/open-feature/go-sdk/openfeature"
)

// toFlagMetadata converts a metadata payload decoded with UseNumber into an
// OpenFeature FlagMetadata map. Absent payloads yield an empty map and
// non-primitive entries are dropped. A new map is returned on every call so
// callers cannot mutate cached state.
func toFlagMetadata(raw map[string]any) openfeature.FlagMetadata {
	out := make(openfeature.FlagMetadata, len(raw))
	for key, value := range raw {
		switch v := value.(type) {
		case string, bool:
			out[key] = v
		case json.Number:
			if i, err := v.Int64(); err == nil {
				out[key] = i
			} else if f, err := v.Float64(); err == nil {
				out[key] = f
			}
		}
	}
	return out
}
