/*
Copyright 2025.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

package hostedcluster

import (
	"sort"

	hyperv1 "github.com/openshift/hypershift/api/hypershift/v1beta1"
)

// BuildServicePublishingStrategy builds the service publishing strategy configuration
// This implementation follows the HyperShift CLI patterns:
//
// LoadBalancer mode (exposeThroughLoadBalancer=true):
// - APIServer: LoadBalancer
// - OAuthServer, Konnectivity, Ignition: Route
// Matches GetIngressServicePublishingStrategyMapping from HyperShift CLI
//
// NodePort mode (exposeThroughLoadBalancer=false):
// - All services (APIServer, OAuthServer, OIDC, Konnectivity, Ignition): NodePort with same address
// Matches GetServicePublishingStrategyMappingByAPIServerAddress from HyperShift CLI
func BuildServicePublishingStrategy(exposeThroughLoadBalancer bool, nodeAddress string) []hyperv1.ServicePublishingStrategyMapping {
	if exposeThroughLoadBalancer {
		// LoadBalancer mode - matches GetIngressServicePublishingStrategyMapping
		return []hyperv1.ServicePublishingStrategyMapping{
			{
				Service: hyperv1.APIServer,
				ServicePublishingStrategy: hyperv1.ServicePublishingStrategy{
					Type: hyperv1.LoadBalancer,
				},
			},
			{
				Service: hyperv1.OAuthServer,
				ServicePublishingStrategy: hyperv1.ServicePublishingStrategy{
					Type: hyperv1.Route,
				},
			},
			{
				Service: hyperv1.Konnectivity,
				ServicePublishingStrategy: hyperv1.ServicePublishingStrategy{
					Type: hyperv1.Route,
				},
			},
			{
				Service: hyperv1.Ignition,
				ServicePublishingStrategy: hyperv1.ServicePublishingStrategy{
					Type: hyperv1.Route,
				},
			},
		}
	}

	// NodePort mode - matches GetServicePublishingStrategyMappingByAPIServerAddress
	services := []hyperv1.ServiceType{
		hyperv1.APIServer,
		hyperv1.OAuthServer,
		hyperv1.OIDC,
		hyperv1.Konnectivity,
		hyperv1.Ignition,
	}

	result := make([]hyperv1.ServicePublishingStrategyMapping, 0, len(services))
	for _, service := range services {
		result = append(result, hyperv1.ServicePublishingStrategyMapping{
			Service: service,
			ServicePublishingStrategy: hyperv1.ServicePublishingStrategy{
				Type: hyperv1.NodePort,
				NodePort: &hyperv1.NodePortPublishingStrategy{
					Address: nodeAddress,
				},
			},
		})
	}

	// Sort by service name for consistency (HyperShift CLI does this)
	sort.Slice(result, func(i, j int) bool {
		return result[i].Service < result[j].Service
	})

	return result
}
