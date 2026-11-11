//! Golden test of `hugepagesQuantity(size, amount)` against Go: fixture
//! `tests/fixtures/quantities.json` from `go run -mod=vendor ./hack/golden/ovshugepages`
//! (`resource.NewQuantity(resource.MustParse(size).Value()*int64(amount),
//! resource.BinarySI).String()`, or the `MustParse` panic message).

use serde::Deserialize;

use crate::ovs_hugepages::hugepages_quantity;

#[derive(Debug, Deserialize)]
struct Case {
    size: String,
    amount: i32,
    quantity: String,
    error: String,
}

#[test]
fn hugepages_quantity_matches_go() {
    let path = format!(
        "{}/tests/fixtures/quantities.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        match hugepages_quantity(&c.size, c.amount) {
            Ok(q) => {
                assert_eq!(c.error, "", "{c:?}: expected error, got {}", q.0);
                assert_eq!(q.0, c.quantity, "{c:?}");
            }
            Err(e) => assert_eq!(e.to_string(), c.error, "{c:?}"),
        }
    }
}
