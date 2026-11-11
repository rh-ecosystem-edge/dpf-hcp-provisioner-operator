//! `ctrlrt-fake` — in-memory fakes for unit tests.
//!
//! * [`FakeClientBuilder`] / [`FakeClient`]: port of controller-runtime's
//!   `sigs.k8s.io/controller-runtime/pkg/client/fake` (v0.22.4), built on a port of
//!   client-go's `testing.ObjectTracker` ([`ObjectTracker`]).
//! * [`FakeClientsetBuilder`] / [`FakeClientset`]: port of `k8s.io/client-go/kubernetes/fake`
//!   (`NewSimpleClientset`) with client-go's reactor chain and action recording.
//!
//! Both expose themselves as a [`ctrlrt::Client`] (via [`ctrlrt::ClientBackend`]); they are
//! cheap to clone, `Send + Sync`, and usable from multiple tokio tasks.
//!
//! ```ignore
//! let scheme = Scheme::new().register::<DPFHCPProvisioner>().register::<Secret>();
//! let c = FakeClientBuilder::new()
//!     .with_scheme(scheme)
//!     .with_objects([obj(&provisioner), obj(&secret)])
//!     .with_status_subresource::<DPFHCPProvisioner>()
//!     .build();
//! ```

pub mod client;
pub mod clientset;
pub mod patch;
pub mod reactor;
pub mod scheme;
pub mod tracker;
mod util;

pub use client::{
    FakeClient, FakeClientBuilder, InitObject, TRACKER_ADD_RESOURCE_VERSION, new_client_builder,
    new_fake_client, obj, obj_mut,
};
pub use clientset::{FakeClientset, FakeClientsetBuilder, new_simple_clientset};
pub use reactor::{Action, Reaction, ReactionFn, verbs};
pub use scheme::{
    CLIENT_GO_SCHEME_NAME, CLIENTSET_FAKE_SCHEME_NAME, NotRegisteredError, Scheme,
    is_not_registered,
};
pub use tracker::{Gvr, ObjectTracker, unsafe_guess_kind_to_resource};
