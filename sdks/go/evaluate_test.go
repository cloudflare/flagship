package flagship

import (
	"bytes"
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"
)

type vectorSuite struct {
	Cases  []vectorCase `json:"cases"`
	Murmur []murmurCase `json:"murmur"`
}

type vectorCase struct {
	Name        string          `json:"name"`
	Definitions json.RawMessage `json:"definitions"`
	AccountID   string          `json:"accountId"`
	FlagKey     string          `json:"flagKey"`
	Context     json.RawMessage `json:"context"`
	Expected    json.RawMessage `json:"expected"`
}

type murmurCase struct {
	Name     string `json:"name"`
	Input    string `json:"input"`
	Seed     uint32 `json:"seed"`
	Expected uint32 `json:"expected"`
}

func loadVectorSuite(t *testing.T) vectorSuite {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	path := filepath.Join(filepath.Dir(thisFile), "..", "..", "spec", "local-evaluation", "cases.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read cases.json: %v", err)
	}
	var suite vectorSuite
	if err := json.Unmarshal(data, &suite); err != nil {
		t.Fatalf("parse cases.json: %v", err)
	}
	return suite
}

func decodeUseNumber(t *testing.T, raw json.RawMessage, dest any) {
	t.Helper()
	dec := json.NewDecoder(bytes.NewReader(raw))
	dec.UseNumber()
	if err := dec.Decode(dest); err != nil {
		t.Fatalf("decode: %v\nraw: %s", err, raw)
	}
}

func TestJSString(t *testing.T) {
	tests := []struct {
		name  string
		value any
		want  string
	}{
		{"true", true, "true"},
		{"false", false, "false"},
		{"int", 1, "1"},
		{"zero", 0, "0"},
		{"neg-zero", math.Copysign(0, -1), "0"},
		{"pos-zero", 0.0, "0"},
		{"one-float", 1.0, "1"},
		{"one-half", 1.5, "1.5"},
		{"1e21", 1e21, "1e+21"},
		{"1e-7", 1e-7, "1e-7"},
		{"1e-6", 1e-6, "0.000001"},
		{"1e20", 1e20, "100000000000000000000"},
		{"inf", math.Inf(1), "Infinity"},
		{"-inf", math.Inf(-1), "-Infinity"},
		{"nan", math.NaN(), "NaN"},
		{"string", "hello", "hello"},
		{"json-number", json.Number("1.5"), "1.5"},
		{"json-number-1e21", json.Number("1e21"), "1e+21"},
		{"null", nil, "null"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := jsString(tt.value)
			if got != tt.want {
				t.Fatalf("jsString(%v) = %q, want %q", tt.value, got, tt.want)
			}
		})
	}
}

func TestJSStringBoolNotInt(t *testing.T) {
	if jsString(true) == jsString(1) {
		t.Fatal("bool true must not stringify like int 1")
	}
}

func TestJSNumber(t *testing.T) {
	tests := []struct {
		name  string
		value any
		want  float64
		nan   bool
	}{
		{"true", true, 1, false},
		{"false", false, 0, false},
		{"int", 42, 42, false},
		{"float", 1.5, 1.5, false},
		{"empty", "", 0, true},
		{"spaces", "   ", 0, true},
		{"tabs", "\t\n", 0, true},
		{"hex", "0x10", 16, false},
		{"octal", "0o10", 8, false},
		{"binary", "0b10", 2, false},
		{"Infinity", "Infinity", math.Inf(1), false},
		{"-Infinity", "-Infinity", math.Inf(-1), false},
		{"inf", "inf", 0, true},
		{"nan-str", "nan", 0, true},
		{"underscore", "1_000", 0, true},
		{"trim", "  42  ", 42, false},
		{"bad", "not-a-number", 0, true},
		{"json-number", json.Number("3.25"), 3.25, false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := jsNumber(tt.value)
			if tt.nan {
				if !math.IsNaN(got) {
					t.Fatalf("jsNumber(%v) = %v, want NaN", tt.value, got)
				}
				return
			}
			if got != tt.want {
				t.Fatalf("jsNumber(%v) = %v, want %v", tt.value, got, tt.want)
			}
		})
	}
}

func TestSharedVectors(t *testing.T) {
	suite := loadVectorSuite(t)
	for _, c := range suite.Cases {
		c := c
		t.Run(c.Name, func(t *testing.T) {
			var defs struct {
				Flags map[string]json.RawMessage `json:"flags"`
			}
			decodeUseNumber(t, c.Definitions, &defs)
			rawFlag, ok := defs.Flags[c.FlagKey]
			if !ok {
				t.Fatalf("flag %q missing from definitions", c.FlagKey)
			}
			flagDef, err := decodeEvalFlag(rawFlag)
			if err != nil {
				t.Fatalf("decode flag: %v", err)
			}

			var context map[string]any
			decodeUseNumber(t, c.Context, &context)

			var expected map[string]any
			decodeUseNumber(t, c.Expected, &expected)

			if _, hasError := expected["error"]; hasError {
				_, err := evaluateFlag(flagDef, context, c.AccountID)
				if err == nil {
					t.Fatal("expected FlagConfigError")
				}
				if _, ok := err.(*flagConfigError); !ok {
					t.Fatalf("err type = %T, want *flagConfigError", err)
				}
				return
			}

			if oneOf, ok := expected["reasonOneOf"].([]any); ok {
				result, err := evaluateFlag(flagDef, context, c.AccountID)
				if err != nil {
					t.Fatal(err)
				}
				for _, candidate := range oneOf {
					if string(result.Reason) == candidate.(string) {
						return
					}
				}
				t.Fatalf("reason %q not in %v", result.Reason, oneOf)
				return
			}

			result, err := evaluateFlag(flagDef, context, c.AccountID)
			if err != nil {
				t.Fatal(err)
			}

			wantValue := expected["value"]
			wantVariant, _ := expected["variant"].(string)
			wantReason, _ := expected["reason"].(string)

			if !valuesEqual(result.Value, wantValue) {
				t.Fatalf("value = %#v (%T), want %#v (%T)", result.Value, result.Value, wantValue, wantValue)
			}
			if result.Variant != wantVariant {
				t.Fatalf("variant = %q, want %q", result.Variant, wantVariant)
			}
			if string(result.Reason) != wantReason {
				t.Fatalf("reason = %q, want %q", result.Reason, wantReason)
			}
		})
	}
}

func TestMurmurVectors(t *testing.T) {
	suite := loadVectorSuite(t)
	for _, m := range suite.Murmur {
		m := m
		t.Run(m.Name, func(t *testing.T) {
			got := murmurhash3(m.Input, m.Seed)
			if got != m.Expected {
				t.Fatalf("murmurhash3(%q, %d) = %d, want %d", m.Input, m.Seed, got, m.Expected)
			}
		})
	}
}

func TestOrdersInfinityViaJSNumber(t *testing.T) {
	flagDef := evalFlag{
		Key:              "inf",
		Enabled:          true,
		DefaultVariation: "none",
		Variations:       map[string]any{"none": "none", "hit": "hit"},
		Rules: []evalRule{{
			Conditions: []any{map[string]any{
				"attribute": "v",
				"operator":  "greater_than",
				"value":     json.Number("1"),
			}},
			ServeVariation: "hit",
		}},
	}
	result, err := evaluateFlag(flagDef, map[string]any{"v": math.Inf(1)}, "acct")
	if err != nil {
		t.Fatal(err)
	}
	if result.Reason != ReasonTargetingMatch || result.Value != "hit" {
		t.Fatalf("result = %#v", result)
	}
}

func valuesEqual(got, want any) bool {
	// json.Number vs float/int from expected comparison.
	switch g := got.(type) {
	case json.Number:
		switch w := want.(type) {
		case json.Number:
			gf, gerr := g.Float64()
			wf, werr := w.Float64()
			if gerr == nil && werr == nil {
				return gf == wf
			}
			return g.String() == w.String()
		case float64:
			gf, err := g.Float64()
			return err == nil && gf == w
		case int:
			gi, err := g.Int64()
			return err == nil && gi == int64(w)
		case int64:
			gi, err := g.Int64()
			return err == nil && gi == w
		}
	}
	return reflect.DeepEqual(got, want)
}
