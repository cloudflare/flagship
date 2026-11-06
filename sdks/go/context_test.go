package flagship

import (
	"math"
	"reflect"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestContextToQueryParamsSerializesPrimitives(t *testing.T) {
	params, err := contextToQueryParams(openfeature.FlattenedContext{
		"targetingKey": "u1",
		"name":         "alice",
		"age":          30,
		"small":        int8(-8),
		"unsigned":     uint16(16),
		"score":        1.5,
		"ratio":        float32(1.25),
		"premium":      true,
		"trial":        false,
	})
	if err != nil {
		t.Fatal(err)
	}

	want := map[string]string{
		"targetingKey": "u1",
		"name":         "alice",
		"age":          "30",
		"small":        "-8",
		"unsigned":     "16",
		"score":        "1.5",
		"ratio":        "1.25",
		"premium":      "true",
		"trial":        "false",
	}
	for key, value := range want {
		if params.Get(key) != value {
			t.Fatalf("params[%q] = %q, want %q", key, params.Get(key), value)
		}
	}
}

func TestNormalizeContextPreservesNil(t *testing.T) {
	normalized, err := normalizeContext(openfeature.FlattenedContext{"a": nil, "b": "x"})
	if err != nil {
		t.Fatal(err)
	}
	if !normalized.requiresPost || normalized.values["a"] != nil || normalized.values["b"] != "x" {
		t.Fatalf("normalized = %#v", normalized)
	}
}

func TestContextToQueryParamsSerializesTime(t *testing.T) {
	ts := time.Date(2024, 1, 2, 3, 4, 5, 600, time.UTC)
	params, err := contextToQueryParams(openfeature.FlattenedContext{"signed_up": ts})
	if err != nil {
		t.Fatal(err)
	}
	if got := params.Get("signed_up"); got != ts.Format(time.RFC3339Nano) {
		t.Fatalf("time = %q, want %q", got, ts.Format(time.RFC3339Nano))
	}
}

func TestNormalizeContextTreatsTypedNilCollectionsAsNull(t *testing.T) {
	var values []string
	var object map[string]any
	normalized, err := normalizeContext(openfeature.FlattenedContext{"values": values, "object": object})
	if err != nil {
		t.Fatal(err)
	}
	if !normalized.requiresPost || normalized.values["values"] != nil || normalized.values["object"] != nil {
		t.Fatalf("normalized = %#v", normalized)
	}
}

func TestNormalizeContextPreservesNestedObjectsArraysAndTimes(t *testing.T) {
	ts := time.Date(2024, 1, 2, 3, 4, 5, 600, time.UTC)
	normalized, err := normalizeContext(openfeature.FlattenedContext{
		"profile": map[string]any{"account": map[string]any{"plan": "enterprise", "createdAt": ts}},
		"tags":    []any{"beta", 42, true, nil},
	})
	if err != nil {
		t.Fatal(err)
	}
	want := map[string]any{
		"profile": map[string]any{"account": map[string]any{"plan": "enterprise", "createdAt": ts.Format(time.RFC3339Nano)}},
		"tags":    []any{"beta", 42, true, nil},
	}
	if !normalized.requiresPost || !reflect.DeepEqual(normalized.values, want) {
		t.Fatalf("normalized = %#v, want %#v", normalized, want)
	}
}

func TestNormalizeContextRejectsCycles(t *testing.T) {
	profile := map[string]any{}
	profile["self"] = profile
	_, err := normalizeContext(openfeature.FlattenedContext{"profile": profile})
	requireFlagshipErrorCode(t, err, ErrorCodeInvalidContext)
}

func TestNormalizeContextRejectsNonFiniteNumbers(t *testing.T) {
	_, err := normalizeContext(openfeature.FlattenedContext{"score": math.NaN()})
	requireFlagshipErrorCode(t, err, ErrorCodeInvalidContext)
}

func TestContextToQueryParamsZeroAndEmptyString(t *testing.T) {
	params, err := contextToQueryParams(openfeature.FlattenedContext{"n": 0, "s": ""})
	if err != nil {
		t.Fatal(err)
	}
	if params.Get("n") != "0" || params.Get("s") != "" {
		t.Fatalf("params = %#v", params)
	}
}
