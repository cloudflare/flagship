package flagship

import (
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestContextToQueryParamsSerializesPrimitives(t *testing.T) {
	params, err := contextToQueryParams(openfeature.FlattenedContext{
		"targetingKey": "u1",
		"name":         "alice",
		"age":          30,
		"score":        1.5,
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
		"score":        "1.5",
		"premium":      "true",
		"trial":        "false",
	}
	for key, value := range want {
		if params.Get(key) != value {
			t.Fatalf("params[%q] = %q, want %q", key, params.Get(key), value)
		}
	}
}

func TestContextToQueryParamsSkipsNil(t *testing.T) {
	params, err := contextToQueryParams(openfeature.FlattenedContext{"a": nil, "b": "x"})
	if err != nil {
		t.Fatal(err)
	}
	if params.Encode() != "b=x" {
		t.Fatalf("params = %q", params.Encode())
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

func TestContextToQueryParamsRejectsComplexValues(t *testing.T) {
	_, err := contextToQueryParams(openfeature.FlattenedContext{"obj": map[string]any{"nested": 1}})
	requireFlagshipErrorCode(t, err, ErrorCodeInvalidContext)
}

func TestContextToQueryParamsRejectsSlices(t *testing.T) {
	_, err := contextToQueryParams(openfeature.FlattenedContext{"arr": []int{1, 2, 3}})
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
