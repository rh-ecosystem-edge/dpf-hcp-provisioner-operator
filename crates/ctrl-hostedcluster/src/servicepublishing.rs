//! Port of internal/controller/hostedcluster/servicepublishing.go

use apis::hypershift::v1beta1 as hyperv1;

fn mapping(
    service: &str,
    strategy: hyperv1::ServicePublishingStrategy,
) -> hyperv1::ServicePublishingStrategyMapping {
    hyperv1::ServicePublishingStrategyMapping {
        service: service.to_string(),
        service_publishing_strategy: strategy,
        ..Default::default()
    }
}

/// BuildServicePublishingStrategy builds the service publishing strategy configuration
/// This implementation follows the HyperShift CLI patterns:
///
/// LoadBalancer mode (exposeThroughLoadBalancer=true):
/// - APIServer: LoadBalancer
/// - OAuthServer, Konnectivity, Ignition: Route
///
/// Matches GetIngressServicePublishingStrategyMapping from HyperShift CLI
///
/// NodePort mode (exposeThroughLoadBalancer=false):
/// - All services (APIServer, OAuthServer, OIDC, Konnectivity, Ignition): NodePort with same address
///
/// Matches GetServicePublishingStrategyMappingByAPIServerAddress from HyperShift CLI
pub fn build_service_publishing_strategy(
    expose_through_load_balancer: bool,
    node_address: &str,
) -> Vec<hyperv1::ServicePublishingStrategyMapping> {
    let typed = |t: &str| hyperv1::ServicePublishingStrategy {
        type_: t.to_string(),
        ..Default::default()
    };

    if expose_through_load_balancer {
        // LoadBalancer mode - matches GetIngressServicePublishingStrategyMapping
        return vec![
            mapping(hyperv1::API_SERVER, typed(hyperv1::LOAD_BALANCER)),
            mapping(hyperv1::OAUTH_SERVER, typed(hyperv1::ROUTE)),
            mapping(hyperv1::KONNECTIVITY, typed(hyperv1::ROUTE)),
            mapping(hyperv1::IGNITION, typed(hyperv1::ROUTE)),
        ];
    }

    // NodePort mode - matches GetServicePublishingStrategyMappingByAPIServerAddress
    let services = [
        hyperv1::API_SERVER,
        hyperv1::OAUTH_SERVER,
        hyperv1::OIDC,
        hyperv1::KONNECTIVITY,
        hyperv1::IGNITION,
    ];

    let mut result: Vec<hyperv1::ServicePublishingStrategyMapping> = services
        .iter()
        .map(|service| {
            mapping(
                service,
                hyperv1::ServicePublishingStrategy {
                    type_: hyperv1::NODE_PORT.to_string(),
                    node_port: Some(hyperv1::NodePortPublishingStrategy {
                        address: node_address.to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
        })
        .collect();

    // Sort by service name for consistency (HyperShift CLI does this)
    // (Go string comparison is bytewise, like Rust's `str` ordering.)
    result.sort_by(|a, b| a.service.cmp(&b.service));

    result
}
