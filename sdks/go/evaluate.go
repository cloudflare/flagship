package flagship

import (
	"encoding/json"
	"fmt"
	"math"
	"math/rand/v2"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// In-process flag evaluation engine.
//
// Port of the Flagship data-plane evaluateFlag. Unexported — not part of the
// public API. Every result must be bit-identical to the server for the same
// definitions and context.

const (
	maxPathDepth      = 5
	hashQuotientRange = (1<<32 + 99) / 100 // ceil(2^32 / 100)
)

var (
	iso8601Regex      = regexp.MustCompile(`^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$`)
	jsWhitespaceRegex = regexp.MustCompile("^[\t\v\f \u00a0\ufeff\n\r\u2028\u2029]+|[\t\v\f \u00a0\ufeff\n\r\u2028\u2029]+$")
)

type evaluationResult struct {
	Value   any
	Variant string
	Reason  EvaluationReason
}

type evalFlag struct {
	Key              string
	Enabled          bool
	DefaultVariation string
	Variations       map[string]any
	Rules            []evalRule
}

type evalRule struct {
	Conditions     []any
	ServeVariation string
	Rollout        *evalRollout
}

type evalRollout struct {
	Percentage float64
	Attribute  string
}

// flagConfigError is raised when a flag definition is internally inconsistent.
type flagConfigError struct {
	flagKey string
	message string
}

func (e *flagConfigError) Error() string {
	return fmt.Sprintf("Flag '%s' %s", e.flagKey, e.message)
}

// jsString matches JavaScript String(value) for scalars used by the engine.
func jsString(value any) string {
	switch v := value.(type) {
	case nil:
		return "null"
	case bool:
		if v {
			return "true"
		}
		return "false"
	case string:
		return v
	case json.Number:
		f, err := v.Float64()
		if err != nil {
			// Fall back to the literal text when it is not a finite float.
			return v.String()
		}
		return jsStringFloat(f)
	case float32:
		return jsStringFloat(float64(v))
	case float64:
		return jsStringFloat(v)
	case int:
		return strconv.Itoa(v)
	case int8:
		return strconv.FormatInt(int64(v), 10)
	case int16:
		return strconv.FormatInt(int64(v), 10)
	case int32:
		return strconv.FormatInt(int64(v), 10)
	case int64:
		return strconv.FormatInt(v, 10)
	case uint:
		return strconv.FormatUint(uint64(v), 10)
	case uint8:
		return strconv.FormatUint(uint64(v), 10)
	case uint16:
		return strconv.FormatUint(uint64(v), 10)
	case uint32:
		return strconv.FormatUint(uint64(v), 10)
	case uint64:
		return strconv.FormatUint(v, 10)
	default:
		return fmt.Sprint(v)
	}
}

func jsStringFloat(n float64) string {
	if math.IsNaN(n) {
		return "NaN"
	}
	if n == 0 {
		// JS String(-0) === "0"
		return "0"
	}
	if math.IsInf(n, 1) {
		return "Infinity"
	}
	if math.IsInf(n, -1) {
		return "-Infinity"
	}

	absN := math.Abs(n)
	// Match Number#toString: scientific form outside [1e-6, 1e21).
	if absN < 1e-6 || absN >= 1e21 {
		return jsExponentString(n)
	}
	return strconv.FormatFloat(n, 'f', -1, 64)
}

// jsExponentString formats n like JS Number#toString in scientific notation
// (e.g. "1e+21", "1.25e-7") rather than Go's zero-padded exponents.
func jsExponentString(n float64) string {
	sign := ""
	if n < 0 {
		sign = "-"
		n = -n
	}

	// Use a wide precision, then trim to the shortest round-trip form JS produces.
	raw := strconv.FormatFloat(n, 'e', -1, 64) // e.g. "1.25e+21" or "1e-07"
	// FormatFloat never includes a leading sign for the magnitude we pass.
	parts := strings.SplitN(raw, "e", 2)
	if len(parts) != 2 {
		return sign + raw
	}
	mant := parts[0]
	// Atoi strips leading zeros ("+07" → 7) so we re-emit JS-style exponents.
	exp, err := strconv.Atoi(parts[1])
	if err != nil {
		return sign + raw
	}
	expStr := strconv.Itoa(exp)
	if exp >= 0 {
		expStr = "+" + expStr
	}
	return sign + mant + "e" + expStr
}

// jsNumber matches JavaScript Number(value) for scalars, with empty-string → NaN.
func jsNumber(value any) float64 {
	switch v := value.(type) {
	case bool:
		if v {
			return 1
		}
		return 0
	case float32:
		return float64(v)
	case float64:
		return v
	case int:
		return float64(v)
	case int8:
		return float64(v)
	case int16:
		return float64(v)
	case int32:
		return float64(v)
	case int64:
		return float64(v)
	case uint:
		return float64(v)
	case uint8:
		return float64(v)
	case uint16:
		return float64(v)
	case uint32:
		return float64(v)
	case uint64:
		return float64(v)
	case json.Number:
		f, err := v.Float64()
		if err != nil {
			return math.NaN()
		}
		return f
	case string:
		return jsNumberString(v)
	default:
		return math.NaN()
	}
}

func jsNumberString(value string) float64 {
	// JS trims whitespace then parses. Whitespace-only → 0 in JS Number(), but
	// the engine's toNumber special-cases empty/whitespace-only as NaN.
	trimmed := jsWhitespaceRegex.ReplaceAllString(value, "")
	if trimmed == "" {
		return math.NaN()
	}

	lower := strings.ToLower(trimmed)
	if lower == "infinity" || lower == "+infinity" {
		return math.Inf(1)
	}
	if lower == "-infinity" {
		return math.Inf(-1)
	}

	// Reject Python/Go-only syntax that JS Number does not accept.
	if strings.Contains(trimmed, "_") {
		return math.NaN()
	}
	if lower == "inf" || lower == "+inf" || lower == "-inf" || lower == "nan" {
		return math.NaN()
	}

	// Hex / octal / binary — JS Number accepts these prefixes (integers only).
	if len(trimmed) >= 2 && trimmed[0] == '0' {
		prefix := trimmed[1]
		if prefix == 'x' || prefix == 'X' || prefix == 'o' || prefix == 'O' || prefix == 'b' || prefix == 'B' {
			// Reject float-looking forms after the prefix (JS does not accept 0x1.0p0).
			n, err := strconv.ParseInt(trimmed, 0, 64)
			if err != nil {
				// Try unsigned for large hex values that still fit float64 mantissa range poorly —
				// ParseUint covers more of the space JS accepts as integers.
				u, uerr := strconv.ParseUint(trimmed, 0, 64)
				if uerr != nil {
					return math.NaN()
				}
				return float64(u)
			}
			return float64(n)
		}
	}

	f, err := strconv.ParseFloat(trimmed, 64)
	if err != nil {
		return math.NaN()
	}
	return f
}

// murmurhash3 is MurmurHash3 x86_32 over UTF-8 bytes. Returns an unsigned 32-bit int.
func murmurhash3(s string, seed uint32) uint32 {
	data := []byte(s) // Go strings are UTF-8; JSON input cannot produce invalid UTF-8.
	n := len(data)
	h := seed
	i := 0
	for i+4 <= n {
		k := uint32(data[i]) | uint32(data[i+1])<<8 | uint32(data[i+2])<<16 | uint32(data[i+3])<<24
		k *= 0xcc9e2d51
		k = (k << 15) | (k >> 17)
		k *= 0x1b873593
		h ^= k
		h = (h << 13) | (h >> 19)
		h = h*5 + 0xe6546b64
		i += 4
	}

	var k uint32
	remaining := n - i
	if remaining >= 3 {
		k ^= uint32(data[i+2]) << 16
	}
	if remaining >= 2 {
		k ^= uint32(data[i+1]) << 8
	}
	if remaining >= 1 {
		k ^= uint32(data[i])
		k *= 0xcc9e2d51
		k = (k << 15) | (k >> 17)
		k *= 0x1b873593
		h ^= k
	}

	h ^= uint32(n)
	h ^= h >> 16
	h *= 0x85ebca6b
	h ^= h >> 13
	h *= 0xc2b2ae35
	h ^= h >> 16
	return h
}

func isScalar(value any) bool {
	switch value.(type) {
	case bool, string, json.Number, float32, float64,
		int, int8, int16, int32, int64,
		uint, uint8, uint16, uint32, uint64:
		return true
	default:
		return false
	}
}

func isPrimitive(value any) bool {
	return value == nil || isScalar(value)
}

// missingAttr is a sentinel distinct from nil (JSON null).
type missingAttr struct{}

var attrMissing = missingAttr{}

func getContextValue(context map[string]any, attribute string) any {
	if value, ok := context[attribute]; ok {
		return value
	}
	if !strings.Contains(attribute, ".") {
		return attrMissing
	}

	path := strings.Split(attribute, ".")
	if len(path) > maxPathDepth {
		return attrMissing
	}

	var value any = context
	for _, segment := range path {
		if isPrimitive(value) {
			return attrMissing
		}
		obj, ok := value.(map[string]any)
		if !ok {
			return attrMissing
		}
		next, ok := obj[segment]
		if !ok {
			return attrMissing
		}
		value = next
	}
	return value
}

func evaluateStringOperator(operator string, attrValue any, target any) bool {
	if attrValue == nil {
		return operator == "not_equals"
	}
	if !isScalar(attrValue) || !isPrimitive(target) {
		return false
	}
	actual := jsString(attrValue)
	expected := jsString(target)

	switch operator {
	case "equals":
		return actual == expected
	case "not_equals":
		return actual != expected
	case "contains":
		return strings.Contains(actual, expected)
	case "starts_with":
		return strings.HasPrefix(actual, expected)
	case "ends_with":
		return strings.HasSuffix(actual, expected)
	default:
		return false
	}
}

func parseISOMs(value string) float64 {
	t, err := time.Parse(time.RFC3339Nano, value)
	if err != nil {
		return math.NaN()
	}
	return float64(t.UTC().UnixMilli())
}

func compareValues(attrValue any, target any) (int, bool) {
	if !isScalar(attrValue) || !isScalar(target) {
		return 0, false
	}

	var actual, expected float64
	targetStr, targetIsStr := target.(string)
	attrStr, attrIsStr := attrValue.(string)
	if targetIsStr && attrIsStr && iso8601Regex.MatchString(targetStr) {
		actual = parseISOMs(attrStr)
		if math.IsNaN(actual) {
			actual = jsNumber(attrValue)
			expected = jsNumber(target)
		} else {
			expected = parseISOMs(targetStr)
		}
	} else {
		actual = jsNumber(attrValue)
		expected = jsNumber(target)
	}

	if math.IsNaN(actual) || math.IsNaN(expected) {
		return 0, false
	}
	if actual == expected {
		return 0, true
	}
	if actual < expected {
		return -1, true
	}
	return 1, true
}

func evaluateOrderingOperator(operator string, attrValue any, target any) bool {
	comparison, ok := compareValues(attrValue, target)
	if !ok {
		return false
	}
	switch operator {
	case "greater_than":
		return comparison > 0
	case "less_than":
		return comparison < 0
	case "greater_than_or_equals":
		return comparison >= 0
	case "less_than_or_equals":
		return comparison <= 0
	default:
		return false
	}
}

func containsValue(values []any, target any) bool {
	expected := jsString(target)
	for _, value := range values {
		if value != nil && jsString(value) == expected {
			return true
		}
	}
	return false
}

func containsPrimitiveValue(values []any, target any) bool {
	expected := jsString(target)
	for _, value := range values {
		if isScalar(value) && jsString(value) == expected {
			return true
		}
	}
	return false
}

func evaluateCondition(condition any, context map[string]any) bool {
	cond, ok := condition.(map[string]any)
	if !ok {
		return false
	}

	if logicalOperator, hasLogical := cond["logical_operator"]; hasLogical {
		clauses, _ := cond["clauses"].([]any)
		op, _ := logicalOperator.(string)
		switch op {
		case "AND":
			for _, clause := range clauses {
				if !evaluateCondition(clause, context) {
					return false
				}
			}
			return true
		case "OR":
			for _, clause := range clauses {
				if evaluateCondition(clause, context) {
					return true
				}
			}
			return false
		default:
			return false
		}
	}

	attribute, _ := cond["attribute"].(string)
	operator, _ := cond["operator"].(string)
	if attribute == "" || operator == "" {
		return false
	}
	target := cond["value"]

	attrValue := getContextValue(context, attribute)
	if _, missing := attrValue.(missingAttr); missing {
		return false
	}

	switch operator {
	case "equals", "not_equals", "contains", "starts_with", "ends_with":
		return evaluateStringOperator(operator, attrValue, target)
	case "greater_than", "less_than", "greater_than_or_equals", "less_than_or_equals":
		return evaluateOrderingOperator(operator, attrValue, target)
	case "in", "not_in":
		targetList, ok := target.([]any)
		if !ok {
			return false
		}
		if attrValue == nil {
			return operator == "not_in"
		}
		if !isScalar(attrValue) {
			return false
		}
		contained := containsValue(targetList, attrValue)
		if operator == "in" {
			return contained
		}
		return !contained
	case "has", "not_has":
		attrList, ok := attrValue.([]any)
		if !ok || !isPrimitive(target) {
			return false
		}
		contained := containsPrimitiveValue(attrList, target)
		if operator == "has" {
			return contained
		}
		return !contained
	default:
		return false
	}
}

// evaluateFlag evaluates a single flag definition against the provided context.
// Pure function — no I/O. Returns flagConfigError when a referenced variation
// is missing from variations.
func evaluateFlag(flagDef evalFlag, context map[string]any, accountID string) (evaluationResult, error) {
	serve := func(variant string, reason EvaluationReason) (evaluationResult, error) {
		if _, ok := flagDef.Variations[variant]; !ok {
			return evaluationResult{}, &flagConfigError{
				flagKey: flagDef.Key,
				message: fmt.Sprintf("variation '%s' is not defined", variant),
			}
		}
		return evaluationResult{
			Value:   flagDef.Variations[variant],
			Variant: variant,
			Reason:  reason,
		}, nil
	}

	if !flagDef.Enabled {
		return serve(flagDef.DefaultVariation, ReasonDisabled)
	}
	if len(flagDef.Rules) == 0 {
		return serve(flagDef.DefaultVariation, ReasonStatic)
	}

	var seed *uint32

	for _, rule := range flagDef.Rules {
		ruleMatches := true
		for _, condition := range rule.Conditions {
			if !evaluateCondition(condition, context) {
				ruleMatches = false
				break
			}
		}

		rollout := rule.Rollout
		isSplit := rollout != nil && rollout.Percentage < 100
		if ruleMatches && rollout != nil {
			attrName := rollout.Attribute
			if attrName == "" {
				attrName = "targetingKey"
			}
			attr := getContextValue(context, attrName)
			_, missing := attr.(missingAttr)
			if !missing && attr != nil && !isScalar(attr) {
				ruleMatches = false
			} else if isSplit {
				if seed == nil {
					s := murmurhash3(accountID+":"+flagDef.Key, 0) % 100
					seed = &s
				}
				var bucket float64
				if !missing && isScalar(attr) {
					h := murmurhash3(jsString(attr), *seed)
					bucket = float64(h%100) + math.Floor(float64(h)/100)/float64(hashQuotientRange)
				} else {
					// Missing or null → random bucket.
					bucket = rand.Float64() * 100
				}
				if bucket >= rollout.Percentage {
					ruleMatches = false
				}
			}
		}

		if ruleMatches {
			reason := ReasonTargetingMatch
			if isSplit {
				reason = ReasonSplit
			}
			return serve(rule.ServeVariation, reason)
		}
	}

	return serve(flagDef.DefaultVariation, ReasonDefault)
}

// decodeEvalFlag parses a single flag entry. Malformed entries return an error
// so the caller can surface PARSE_ERROR without invalidating the snapshot.
func decodeEvalFlag(raw json.RawMessage) (evalFlag, error) {
	decoder := json.NewDecoder(strings.NewReader(string(raw)))
	decoder.UseNumber()

	var payload map[string]any
	if err := decoder.Decode(&payload); err != nil {
		return evalFlag{}, fmt.Errorf("invalid flag JSON: %w", err)
	}

	key, _ := payload["key"].(string)
	enabled, _ := payload["enabled"].(bool)
	defaultVariation, _ := payload["default_variation"].(string)

	variations := map[string]any{}
	if rawVariations, ok := payload["variations"].(map[string]any); ok {
		variations = rawVariations
	}

	var rules []evalRule
	if rawRules, ok := payload["rules"].([]any); ok {
		rules = make([]evalRule, 0, len(rawRules))
		for _, rawRule := range rawRules {
			ruleMap, ok := rawRule.(map[string]any)
			if !ok {
				continue
			}
			rule := evalRule{}
			if conditions, ok := ruleMap["conditions"].([]any); ok {
				rule.Conditions = conditions
			}
			if serveVariation, ok := ruleMap["serve_variation"].(string); ok {
				rule.ServeVariation = serveVariation
			} else if serveVariation, ok := ruleMap["serve_variation"]; ok {
				rule.ServeVariation = jsString(serveVariation)
			}
			if rolloutRaw, ok := ruleMap["rollout"].(map[string]any); ok {
				rollout := &evalRollout{}
				if pct, ok := rolloutRaw["percentage"]; ok {
					rollout.Percentage = jsNumber(pct)
				} else {
					rollout.Percentage = 100
				}
				if attr, ok := rolloutRaw["attribute"].(string); ok {
					rollout.Attribute = attr
				}
				rule.Rollout = rollout
			}
			rules = append(rules, rule)
		}
	}

	return evalFlag{
		Key:              key,
		Enabled:          enabled,
		DefaultVariation: defaultVariation,
		Variations:       variations,
		Rules:            rules,
	}, nil
}
