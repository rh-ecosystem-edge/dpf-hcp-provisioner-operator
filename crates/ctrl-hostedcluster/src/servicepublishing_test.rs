//! Port of internal/controller/hostedcluster/servicepublishing_test.go
//! ("Service Publishing Strategy Builder").

use apis::hypershift::v1beta1 as hyperv1;

use crate::*;

/// Helper function to find strategy for a specific service
fn find_service_strategy_by_type<'a>(
    strategies: &'a [hyperv1::ServicePublishingStrategyMapping],
    service: &str,
) -> Option<&'a hyperv1::ServicePublishingStrategy> {
    strategies
        .iter()
        .find(|s| s.service == service)
        .map(|s| &s.service_publishing_strategy)
}

// Context("LoadBalancer Mode")

#[test]
fn service_publishing_strategy_builder_load_balancer_mode_should_return_4_service_publishing_strategies()
 {
    let strategy = build_service_publishing_strategy(true, "");
    assert_eq!(strategy.len(), 4);
}

#[test]
fn service_publishing_strategy_builder_load_balancer_mode_should_use_load_balancer_for_api_server()
{
    let strategy = build_service_publishing_strategy(true, "");
    let s = find_service_strategy_by_type(&strategy, hyperv1::API_SERVER).expect("strategy");
    assert_eq!(s.type_, hyperv1::LOAD_BALANCER);
}

#[test]
fn service_publishing_strategy_builder_load_balancer_mode_should_use_route_for_oauth_server() {
    let strategy = build_service_publishing_strategy(true, "");
    let s = find_service_strategy_by_type(&strategy, hyperv1::OAUTH_SERVER).expect("strategy");
    assert_eq!(s.type_, hyperv1::ROUTE);
}

#[test]
fn service_publishing_strategy_builder_load_balancer_mode_should_use_route_for_konnectivity() {
    let strategy = build_service_publishing_strategy(true, "");
    let s = find_service_strategy_by_type(&strategy, hyperv1::KONNECTIVITY).expect("strategy");
    assert_eq!(s.type_, hyperv1::ROUTE);
}

#[test]
fn service_publishing_strategy_builder_load_balancer_mode_should_use_route_for_ignition() {
    let strategy = build_service_publishing_strategy(true, "");
    let s = find_service_strategy_by_type(&strategy, hyperv1::IGNITION).expect("strategy");
    assert_eq!(s.type_, hyperv1::ROUTE);
}

// Context("NodePort Mode")

const NODE_ADDRESS: &str = "192.168.1.100";

fn assert_node_port(service: &str) {
    let strategy = build_service_publishing_strategy(false, NODE_ADDRESS);
    let s = find_service_strategy_by_type(&strategy, service).expect("strategy");
    assert_eq!(s.type_, hyperv1::NODE_PORT);
    let np = s.node_port.as_ref().expect("nodePort");
    assert_eq!(np.address, NODE_ADDRESS);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_return_5_service_publishing_strategies_including_oidc()
 {
    let strategy = build_service_publishing_strategy(false, NODE_ADDRESS);
    assert_eq!(strategy.len(), 5);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_api_server_with_correct_address()
 {
    assert_node_port(hyperv1::API_SERVER);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_oauth_server() {
    assert_node_port(hyperv1::OAUTH_SERVER);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_oidc() {
    assert_node_port(hyperv1::OIDC);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_konnectivity() {
    assert_node_port(hyperv1::KONNECTIVITY);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_use_node_port_for_ignition() {
    assert_node_port(hyperv1::IGNITION);
}

#[test]
fn service_publishing_strategy_builder_node_port_mode_should_sort_services_alphabetically() {
    let strategy = build_service_publishing_strategy(false, NODE_ADDRESS);

    // Verify services are in alphabetical order
    for w in strategy.windows(2) {
        let current = &w[0].service;
        let next = &w[1].service;
        assert!(current < next, "Expected {current} to be before {next}");
    }
}
