// Package conformance runs the official Gateway API conformance test suite
// against the Zentinel gateway controller.
//
// Run with:
//
//	go test ./... -run TestConformance -gateway-class=zentinel -v
package conformance

import (
	"testing"

	"sigs.k8s.io/gateway-api/conformance"
	"sigs.k8s.io/gateway-api/conformance/utils/suite"
)

func TestConformance(t *testing.T) {
	opts := conformance.DefaultOptions(t)

	// Override gateway class if not set via flags
	if opts.GatewayClassName == "" {
		opts.GatewayClassName = "zentinel"
	}

	// Enable the Gateway HTTP conformance profile
	opts.ConformanceProfiles.Insert(suite.GatewayHTTPConformanceProfileName)

	conformance.RunConformanceWithOptions(t, opts)
}
