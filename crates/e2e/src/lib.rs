//! Support library for the end-to-end suite (`tests/e2e_suite`):
//!
//! * [`utils`] — port of `test/utils/utils.go` (`Run`, `GetProjectDir`, `UncommentCode`, ...).
//! * [`ginkgo`] — the subset of Ginkgo v2 the suite uses (ordered containers, setup nodes,
//!   labels/focus filtering, suite timeout, `By`, `GinkgoWriter`).
//! * [`gomega`] — the Gomega matchers, `Eventually` and `Consistently`.

pub mod ginkgo;
pub mod gomega;
pub mod utils;
