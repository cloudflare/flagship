// Example: Local in-process evaluation with Flagship ServerProvider.
//
// Downloads flag definitions once, evaluates flags without a network call per
// evaluation, and refreshes definitions in the background.
//
// Requires a token with app **read** permission (not evaluate). Local evaluations
// do not appear in server-side analytics.
package main

import (
	"context"
	"log"
	"time"

	flagship "github.com/cloudflare/flagship/sdks/go"
	"github.com/open-feature/go-sdk/openfeature"
)

func main() {
	ctx := context.Background()

	provider, err := flagship.NewProvider(flagship.Options{
		AppID:           "your-app-id",
		AccountID:       "your-account-id", // required — used as the rollout hash seed
		AuthToken:       "your-read-token", // needs app **read** permission
		LocalEvaluation: true,
		// Background definitions refresh period (default 30s).
		RefreshInterval: 30 * time.Second,
		Logging:         true,
	})
	if err != nil {
		log.Fatal(err)
	}

	// SetProviderAndWait blocks until the first definitions fetch succeeds.
	if err := openfeature.SetProviderAndWait(provider); err != nil {
		log.Fatal(err)
	}
	defer openfeature.Shutdown()

	client := openfeature.NewDefaultClient()
	evalCtx := openfeature.NewEvaluationContext("user-123", map[string]any{
		"email": "user@example.com",
		"plan":  "premium",
	})

	darkMode, err := client.BooleanValue(ctx, "dark-mode", false, evalCtx)
	if err != nil {
		log.Fatal(err)
	}
	log.Println("Dark mode:", darkMode)

	details, err := client.BooleanValueDetails(ctx, "premium-features", false, evalCtx)
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("Premium features: value=%v reason=%s variant=%s", details.Value, details.Reason, details.Variant)
}
