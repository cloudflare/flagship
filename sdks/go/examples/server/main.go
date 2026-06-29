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
		AppID:     "your-app-id",
		AccountID: "your-account-id",
		AuthToken: "your-token",
		CacheTTL:  30 * time.Second, // cache evaluations per context for 30s (off by default)
	})
	if err != nil {
		log.Fatal(err)
	}

	if err := openfeature.SetProviderAndWait(provider); err != nil {
		log.Fatal(err)
	}
	defer openfeature.Shutdown()

	client := openfeature.NewDefaultClient()
	evalCtx := openfeature.NewEvaluationContext("user-123", map[string]any{
		"plan": "premium",
	})

	enabled, err := client.BooleanValue(ctx, "dark-mode", false, evalCtx)
	if err != nil {
		log.Fatal(err)
	}

	log.Println("dark-mode:", enabled)
}
